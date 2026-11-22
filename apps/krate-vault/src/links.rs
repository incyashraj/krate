//! What a press or a key in the note does beyond typing: follow a link to
//! its heading or block, offer a note's headings after `[[Note#` and its
//! blocks after `[[Note#^`, tick a task or a checkbox property, fold, add
//! carets, and drive the find bar. The guest decides what was pressed
//! (`guest/editor.rs`); this decides what that does to the app.

use crate::app::{App, Completion, Focus, Mods};
use crate::buffer::Pos;
use crate::editor::{Doc, Mode};
use crate::find;
use crate::text::lower;
use alloc::string::String;
use alloc::vec::Vec;

/// What a press in the note lands on.
#[derive(Debug, Clone, PartialEq)]
pub enum Act {
    /// A `[[link]]`, with any `#heading` or `#^block` after the name.
    Follow(String),
    Url(String),
    Tag(String),
    /// Tick the task on this line.
    Task(usize),
    /// Fold or unfold the callout whose head is on this line.
    Callout(usize),
    /// Fold or unfold the heading or list item on this line.
    Fold(usize),
    /// Tick the checkbox property on this line.
    Property(usize),
    /// Add a property to the front matter.
    AddProperty,
    /// Put the caret here (a click into a block drawn whole shows its
    /// source there).
    Place(Pos),
    /// A press on the find bar.
    Find(FindHit),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FindHit {
    Bar,
    Query,
    Replace,
    Prev,
    Next,
    ToggleReplace,
    ReplaceOne,
    ReplaceAll,
    Close,
    Case,
    Word,
    Regex,
}

/// Split `Note#Heading` (or `Note#^block`) into the note and the anchor.
pub fn split_anchor(full: &str) -> (&str, Option<&str>) {
    match full.find('#') {
        Some(i) => (
            full.get(..i).unwrap_or("").trim(),
            Some(full.get(i + 1..).unwrap_or("").trim()).filter(|a| !a.is_empty()),
        ),
        None => (full.trim(), None),
    }
}

/// Follow a link: open the note (making it when it is missing), then go to
/// its heading or block. `[[#Heading]]` stays in this note.
pub fn follow(app: &mut App, full: &str, new_tab: bool) {
    let (name, anchor) = split_anchor(full);
    if !name.is_empty() {
        app.follow(name, new_tab);
    }
    if let Some(a) = anchor {
        if let Some(d) = app.doc_mut() {
            if let Some(n) = d.anchor_line(a) {
                d.jump_to(n);
            }
        }
    }
    app.focus = Focus::Editor;
}

/// Do what a press landed on. True when it was handled.
pub fn act_on(app: &mut App, act: Act, m: Mods) -> bool {
    let now = app.now;
    match act {
        Act::Follow(full) => follow(app, &full, m.primary),
        Act::Url(_) => return false,
        Act::Tag(t) => app.search_tag(&t),
        Act::Find(h) => {
            let Some(d) = app.doc_mut() else {
                return false;
            };
            match h {
                FindHit::Bar => {}
                FindHit::Query | FindHit::Replace => {
                    if let Some(f) = d.find.as_mut() {
                        f.focused = true;
                        f.in_replace = h == FindHit::Replace;
                    }
                }
                FindHit::Prev => find::step(d, true),
                FindHit::Next => find::step(d, false),
                FindHit::ToggleReplace => {
                    if let Some(f) = d.find.as_mut() {
                        f.replacing = !f.replacing && d.mode != Mode::Reading;
                        f.in_replace = f.replacing;
                        f.focused = true;
                    }
                }
                FindHit::ReplaceOne => find::replace_one(d, now),
                FindHit::ReplaceAll => {
                    find::replace_all(d, now);
                }
                FindHit::Close => find::close(d),
                FindHit::Case | FindHit::Word | FindHit::Regex => {
                    if let Some(f) = d.find.as_mut() {
                        match h {
                            FindHit::Case => f.case = !f.case,
                            FindHit::Word => f.word = !f.word,
                            _ => f.regex = !f.regex,
                        }
                        f.focused = true;
                    }
                    find::requery(d);
                }
            }
        }
        other => {
            let Some(d) = app.doc_mut() else {
                return false;
            };
            if let Some(f) = d.find.as_mut() {
                f.focused = false;
            }
            let reading = d.mode == Mode::Reading;
            match other {
                Act::Task(n) if !reading => d.toggle_task(n, now),
                Act::Callout(n) => d.toggle_callout(n),
                Act::Fold(n) => {
                    d.toggle_fold(n);
                }
                Act::Property(n) if !reading => toggle_property(d, n, now),
                Act::AddProperty if !reading => add_property(d, now),
                Act::Place(p) if !reading => {
                    d.extra.clear();
                    d.place(p, false);
                }
                _ => return reading,
            }
            app.focus = Focus::Editor;
        }
    }
    true
}

/// Tick or untick a checkbox property.
pub fn toggle_property(d: &mut Doc, n: usize, now: u64) {
    let line = String::from(d.line(n));
    if let Some(new) = crate::props::toggled_line(&line) {
        d.replace(Pos::new(n, 0), Pos::new(n, line.len()), &new, now);
    }
}

/// Add a property at the end of the front matter (or make the front
/// matter) and put the caret on its value.
pub fn add_property(d: &mut Doc, now: u64) {
    let (at, existing) = match d.front {
        Some(end) => {
            let lines: Vec<&str> = (1..end).map(|n| d.line(n)).collect();
            (end, crate::props::parse(&lines, 1))
        }
        None => {
            d.replace(Pos::new(0, 0), Pos::new(0, 0), "---\n---\n", now);
            (1, Vec::new())
        }
    };
    let mut key = crate::props::new_key(&existing);
    key.push_str(": \n");
    d.replace(Pos::new(at, 0), Pos::new(at, 0), &key, now);
    let len = d.buf.line_len(at);
    d.extra.clear();
    d.select_range(Pos::new(at, 0), Pos::new(at, len.saturating_sub(2)));
}

/// Choosing completion item `i`, when it is a block: write its new `^id`
/// into the target note (an open tab's text, or the file), then insert the
/// link. True when it was a block item.
pub fn accept_block(app: &mut App, i: usize) -> bool {
    let Some(data) = app
        .completion
        .as_ref()
        .and_then(|c| c.items.get(i))
        .map(|(_, d)| d.clone())
        .filter(|d| d.starts_with(BLOCK_MARK))
    else {
        return false;
    };
    let mut parts = data.split(BLOCK_MARK).skip(1);
    let ins = String::from(parts.next().unwrap_or(""));
    let path = parts.next().unwrap_or("").replace('\u{2}', "/");
    let write: Option<usize> = parts.next().and_then(|w| w.parse().ok());
    let id = ins.rsplit("#^").next().unwrap_or("");
    let now = app.now;
    if let Some(line) = write {
        let mut tag = String::from(" ^");
        tag.push_str(id);
        let mut done = false;
        let tabs = app
            .tabs
            .iter_mut()
            .chain(app.panes.iter_mut().flat_map(|p| p.tabs.iter_mut()));
        for t in tabs {
            if let Some(d) = t.doc.as_mut().filter(|d| d.path == path) {
                let end = Pos::new(line, d.buf.line_len(line));
                let (cur, anchor) = (d.cur, d.anchor);
                d.replace(end, end, &tag, now);
                d.cur = cur;
                d.anchor = anchor;
                done = true;
            }
        }
        if !done {
            if let Some(v) = app.vault.as_mut() {
                if let Ok(text) = crate::app::read_note(v, &path) {
                    let mut lines: Vec<String> = text.split('\n').map(String::from).collect();
                    if let Some(l) = lines.get_mut(line) {
                        l.push_str(&tag);
                    }
                    let _ = crate::app::write_note(v, &path, &lines.join("\n"));
                }
            }
        }
    }
    if let Some(d) = app.doc_mut() {
        d.accept_link(&ins, now);
    }
    app.completion = None;
    true
}

/// `[[` completion for what follows a `#`: the note's headings, or after
/// `#^` its blocks with ids. True when it answered (the app's own name
/// completion is not wanted then).
pub fn anchor_completion(app: &mut App) -> bool {
    let query = match app.doc() {
        Some(d) if d.mode != Mode::Reading && app.focus == Focus::Editor => d.link_query(),
        _ => None,
    };
    let Some((_, q)) = query else {
        return false;
    };
    let Some(hash) = q.find('#') else {
        return false;
    };
    if app.completion.as_ref().is_some_and(|c| c.query == q) {
        return true;
    }
    let note = q.get(..hash).unwrap_or("").trim();
    let rest = q.get(hash + 1..).unwrap_or("");
    let path: Option<String> = if note.is_empty() {
        app.doc().map(|d| d.path.clone())
    } else {
        app.vault.as_ref().and_then(|v| {
            v.resolve(note)
                .and_then(|i| v.notes.get(i))
                .map(|n| n.path.clone())
        })
    };
    let text: Option<String> = match &path {
        Some(p) => app.open_text(p).or_else(|| {
            app.vault
                .as_mut()
                .and_then(|v| crate::app::read_note(v, p).ok())
        }),
        None => None,
    };
    let text = text.unwrap_or_default();
    let mut items = anchor_items(note, rest, &text, path.as_deref().unwrap_or(""), app.now);
    items.truncate(30);
    app.completion = Some(Completion {
        query: q,
        items,
        sel: 0,
    });
    true
}

/// The headings (or, after `^`, the block ids) of `text` that match what
/// was typed, as (what to insert, a label to show beside it).
/// Marks a block completion item; its parts are separated by it.
const BLOCK_MARK: char = '\u{1}';

/// A new block id: six letters and digits, from the place and the time.
pub fn new_block_id(path: &str, line: usize, seed: u64) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ seed;
    for b in path.bytes().chain(line.to_le_bytes()) {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01B3);
    }
    let digits = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut s = String::new();
    for _ in 0..6 {
        s.push(char::from(digits[(h % 36) as usize]));
        h /= 36;
    }
    s
}

/// The headings (or, after `^`, the blocks) of `text` that match what was
/// typed, as (what to insert or show, a label or what choosing it does).
pub fn anchor_items(
    note: &str,
    typed: &str,
    text: &str,
    path: &str,
    seed: u64,
) -> Vec<(String, String)> {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut out: Vec<(i32, String, String)> = Vec::new();
    if let Some(want) = typed.strip_prefix('^') {
        // Every block of the note: those with an id link to it, the rest
        // get one written in when chosen, as Obsidian does.
        let mut in_code = false;
        let front =
            crate::blocks::front_matter_end(|n| lines.get(n).copied().unwrap_or(""), lines.len());
        for (n, l) in lines.iter().enumerate() {
            if front.is_some_and(|f| n <= f) {
                continue;
            }
            if crate::md::is_fence(l) {
                in_code = !in_code;
                continue;
            }
            let next_blank = lines.get(n + 1).is_none_or(|x| {
                let nb = crate::md::classify(x, false);
                x.trim().is_empty()
                    || nb.is_list()
                    || matches!(
                        nb,
                        crate::md::Block::Heading { .. } | crate::md::Block::Fence
                    )
            });
            let block = crate::md::classify(l, in_code);
            // A block ends at a blank line, or is one line (list items,
            // headings).
            let ends = !in_code
                && block != crate::md::Block::Blank
                && (next_blank
                    || block.is_list()
                    || matches!(block, crate::md::Block::Heading { .. }));
            if !ends {
                continue;
            }
            let t = l.trim_end();
            let (id, text) = match t.rfind(" ^") {
                Some(i)
                    if t.get(i + 2..).is_some_and(|x| {
                        !x.is_empty() && x.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                    }) =>
                {
                    (
                        Some(String::from(t.get(i + 2..).unwrap_or(""))),
                        t.get(..i).unwrap_or(""),
                    )
                }
                _ => (None, t),
            };
            let shown: String = crate::md::plain(text, block.content())
                .trim()
                .chars()
                .take(70)
                .collect();
            if shown.is_empty() {
                continue;
            }
            let score = if want.is_empty() {
                Some(0)
            } else {
                crate::text::fuzzy(&lower(want), &shown).or_else(|| {
                    id.as_ref()
                        .and_then(|i| crate::text::fuzzy(&lower(want), i))
                })
            };
            let Some(sc) = score else {
                continue;
            };
            let (the_id, write) = match id {
                Some(i) => (i, String::new()),
                None => {
                    let mut w = String::new();
                    crate::text::push_num(&mut w, n);
                    (new_block_id(path, n, seed), w)
                }
            };
            let mut ins = String::from(note);
            ins.push_str("#^");
            ins.push_str(&the_id);
            // (what the list shows, what choosing it does), the second
            // starting with a mark so accept_block knows it.
            let mut data = String::from(BLOCK_MARK);
            data.push_str(&ins);
            data.push(BLOCK_MARK);
            data.push_str(&path.replace('/', "\u{2}"));
            data.push(BLOCK_MARK);
            data.push_str(&write);
            out.push((sc, shown, data));
        }
    } else {
        let mut in_code = false;
        for l in &lines {
            if crate::md::is_fence(l) {
                in_code = !in_code;
                continue;
            }
            if in_code {
                continue;
            }
            if let crate::md::Block::Heading { level, content } = crate::md::classify(l, false) {
                let h = crate::md::plain(l, content);
                let score = if typed.trim().is_empty() {
                    Some(0)
                } else {
                    crate::text::fuzzy(&lower(typed.trim()), &h)
                };
                if let Some(s) = score {
                    let mut ins = String::from(note);
                    ins.push('#');
                    ins.push_str(&h);
                    let mut label = String::from("H");
                    crate::text::push_num(&mut label, level as usize);
                    label.push('/');
                    out.push((s, ins, label));
                }
            }
        }
    }
    if !typed.trim_start_matches('^').is_empty() {
        out.sort_by_key(|a| core::cmp::Reverse(a.0));
    }
    out.into_iter().map(|(_, a, b)| (a, b)).collect()
}

/// Keys before the app's own: the find bar, several carets, folding.
/// `None` lets the app handle the key.
pub fn key(app: &mut App, key: &str, m: Mods) -> Option<bool> {
    if app.overlay.is_some() || app.explorer.rename.is_some() || app.focus != Focus::Editor {
        return None;
    }
    if let Some(used) = find::key(app, key, m) {
        return Some(used);
    }
    let now = app.now;
    let completing = app.completion.is_some();
    let d = app.doc_mut()?;
    let k = lower(key);
    let reading = d.mode == Mode::Reading;
    // Folding: Cmd+Option+[ folds, Cmd+Option+] unfolds (the editor's own
    // keys); with Shift, every heading and list at once.
    // Option+[ and Option+] arrive as the quotes a Mac types with them.
    let open_b = matches!(k.as_str(), "[" | "bracketleft" | "“");
    let close_b = matches!(k.as_str(), "]" | "bracketright" | "‘");
    if m.primary && m.alt && (open_b || close_b) {
        let fold = open_b;
        if m.shift {
            d.fold_all(fold);
        } else {
            let n = d.cur.line;
            let folded = d.folds.contains(&n);
            if fold != folded {
                d.toggle_fold(n);
            }
        }
        return Some(true);
    }
    if reading {
        return None;
    }
    if m.primary && !m.alt && !m.shift && k == "d" {
        d.select_next_occurrence();
        return Some(true);
    }
    if m.primary && m.alt && (key == "ArrowUp" || key == "ArrowDown") {
        d.add_caret_vertical(key == "ArrowDown");
        return Some(true);
    }
    // In a table: cell to cell (tables.rs).
    if d.mode == Mode::Live
        && !d.has_extra()
        && !completing
        && !m.primary
        && !m.alt
        && crate::tables::key(d, key, m.shift, now).is_some()
    {
        return Some(true);
    }
    if !d.has_extra() || completing {
        return None;
    }
    match key {
        "Escape" => {
            d.extra.clear();
            d.anchor = None;
        }
        "Backspace" => d.each_caret(now, |d, now| d.backspace(now)),
        "Delete" => d.each_caret(now, |d, now| d.delete_forward(now)),
        "Enter" => d.each_caret(now, |d, now| d.newline(now)),
        "ArrowLeft" if !m.primary && !m.alt => d.each_caret(now, |d, _| d.left(m.shift)),
        "ArrowRight" if !m.primary && !m.alt => d.each_caret(now, |d, _| d.right(m.shift)),
        "Home" => d.each_caret(now, |d, _| {
            let p = Pos::new(d.cur.line, 0);
            d.place(p, m.shift)
        }),
        "End" => d.each_caret(now, |d, _| {
            let p = Pos::new(d.cur.line, d.buf.line_len(d.cur.line));
            d.place(p, m.shift)
        }),
        _ => return None,
    }
    Some(true)
}

/// Typed text before the app's own handling: the find bar's, or every
/// caret's when there are several. True when it was taken.
pub fn text(app: &mut App, s: &str) -> bool {
    if find::text(app, s) {
        return true;
    }
    let now = app.now;
    let Some(d) = app.doc_mut() else {
        return false;
    };
    if d.mode == Mode::Live && !d.has_extra() {
        if s == "\n" && crate::tables::key(d, "Enter", false, now).is_some() {
            return true;
        }
        if let Some(esc) = crate::tables::typed(d, s) {
            d.insert(&esc, now);
            return true;
        }
        // Plain typing in a table still lands inside a cell.
        crate::tables::snap(d);
    }
    if !d.has_extra() || d.mode == Mode::Reading {
        return false;
    }
    let clean: String = s
        .chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect();
    if clean.is_empty() {
        return true;
    }
    d.each_caret(now, |d, now| {
        if clean == "\n" {
            d.newline(now);
        } else {
            d.insert(&clean, now);
        }
    });
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::mem_vault;

    fn app_with(files: &[(&str, &str)]) -> App {
        let mut a = App::new();
        a.open_vault(mem_vault("v", files));
        a
    }

    #[test]
    fn links_go_to_their_heading_and_block() {
        let target = "# Target\nintro\n\n## Deep part\nline\nA block here ^b1\n";
        let mut a = app_with(&[("Home.md", "[[Target#Deep part]]"), ("Target.md", target)]);
        a.open("Home.md", false);
        follow(&mut a, "Target#Deep part", false);
        let d = a.doc().unwrap();
        assert_eq!(d.path, "Target.md");
        assert_eq!(d.cur.line, 3);
        assert_eq!(d.top, 4, "the heading is at the top of the view");
        follow(&mut a, "Target#^b1", false);
        assert_eq!(a.doc().unwrap().cur.line, 5);
        follow(&mut a, "#Target", false);
        assert_eq!(
            a.doc().unwrap().cur.line,
            0,
            "a link to a heading in the same note"
        );
        assert_eq!(split_anchor("Note#^x"), ("Note", Some("^x")));
    }

    #[test]
    fn headings_and_blocks_complete_after_a_hash() {
        let text = "# One\n## Two things\n```\n# not\n```\nPara ^p1\n- item ^li-2\n";
        let h = anchor_items("Note", "", text, "Note.md", 7);
        assert_eq!(
            h,
            [
                (String::from("Note#One"), String::from("H1/")),
                (String::from("Note#Two things"), String::from("H2/")),
            ]
        );
        let t = anchor_items("Note", "thi", text, "Note.md", 7);
        assert_eq!(t[0].0, "Note#Two things");
        let b = anchor_items("Note", "^", text, "Note.md", 7);
        let shown: Vec<&str> = b.iter().map(|x| x.0.as_str()).collect();
        assert_eq!(shown, ["One", "Two things", "Para", "item"]);
        assert!(b[2].1.contains("Note#^p1"));
        // Choosing a block with no id writes one into the note.
        let mut w = app_with(&[("Home.md", ""), ("Note.md", "intro\n\nNo id here\n")]);
        w.open("Home.md", false);
        let now = w.now;
        w.doc_mut().unwrap().insert("[[Note#^no id", now);
        assert!(anchor_completion(&mut w));
        assert_eq!(w.completion.as_ref().unwrap().items[0].0, "No id here");
        w.accept_completion(0);
        let link = w.doc().unwrap().text();
        let id = link
            .trim_start_matches("[[Note#^")
            .trim_end_matches("]]")
            .to_string();
        assert_eq!(id.len(), 6);
        let note = crate::app::read_note(w.vault.as_mut().unwrap(), "Note.md").unwrap();
        assert!(note.contains(&alloc::format!("No id here ^{id}")), "{note}");
        let mut a = app_with(&[("Home.md", ""), ("Note.md", text)]);
        a.open("Home.md", false);
        let now = a.now;
        a.doc_mut().unwrap().insert("[[Note#Tw", now);
        assert!(anchor_completion(&mut a));
        let c = a.completion.as_ref().unwrap();
        assert_eq!(c.items[0].0, "Note#Two things");
        a.accept_completion(0);
        assert_eq!(a.doc().unwrap().text(), "[[Note#Two things]]");
    }

    #[test]
    fn presses_tick_fold_and_edit_properties() {
        let text = "---\ndone: false\n---\n# H\n- [ ] task\nbody";
        let mut a = app_with(&[("N.md", text)]);
        a.open("N.md", false);
        let m = Mods::default();
        assert!(act_on(&mut a, Act::Property(1), m));
        assert_eq!(a.doc().unwrap().line(1), "done: true");
        act_on(&mut a, Act::Task(4), m);
        assert_eq!(a.doc().unwrap().line(4), "- [x] task");
        act_on(&mut a, Act::Fold(3), m);
        assert!(a.doc().unwrap().folded_away(5));
        act_on(&mut a, Act::AddProperty, m);
        let d = a.doc().unwrap();
        assert_eq!(d.line(2), "property: ");
        assert_eq!(d.selected_text(), "property");
        let mut b = app_with(&[("M.md", "plain")]);
        b.open("M.md", false);
        add_property(b.doc_mut().unwrap(), 0);
        assert_eq!(b.doc().unwrap().text(), "---\nproperty: \n---\nplain");
    }

    #[test]
    fn keys_drive_several_carets_and_the_find_bar() {
        let mut a = app_with(&[("N.md", "cat\ncat\ndog")]);
        a.open("N.md", false);
        let cmd = Mods {
            primary: true,
            ..Mods::default()
        };
        assert_eq!(key(&mut a, "d", cmd), Some(true));
        assert_eq!(key(&mut a, "d", cmd), Some(true));
        assert!(text(&mut a, "cow"));
        assert_eq!(a.doc().unwrap().text(), "cow\ncow\ndog");
        assert_eq!(key(&mut a, "Escape", Mods::default()), Some(true));
        assert!(!a.doc().unwrap().has_extra());
        assert_eq!(key(&mut a, "f", cmd), Some(true));
        assert!(text(&mut a, "dog"));
        assert_eq!(a.doc().unwrap().selected_text(), "dog");
        assert_eq!(key(&mut a, "Escape", Mods::default()), Some(true));
        assert!(a.doc().unwrap().find.is_none());
        // Cmd+Option+F opens it with the replace row, whether the host sends
        // "f" or the "ƒ" Option makes on a Mac.
        let cmd_opt = Mods {
            primary: true,
            alt: true,
            ..Mods::default()
        };
        for k in ["f", "ƒ"] {
            assert_eq!(key(&mut a, k, cmd_opt), Some(true));
            assert!(a.doc().unwrap().find.as_ref().is_some_and(|f| f.replacing));
            assert_eq!(key(&mut a, "Escape", Mods::default()), Some(true));
        }
    }
}
