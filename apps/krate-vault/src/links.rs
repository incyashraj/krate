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
    let text: Option<String> = if note.is_empty() {
        app.doc().map(Doc::text)
    } else {
        let path = app.vault.as_ref().and_then(|v| {
            v.resolve(note)
                .and_then(|i| v.notes.get(i))
                .map(|n| n.path.clone())
        });
        match path {
            Some(p) => app.open_text(&p).or_else(|| {
                app.vault
                    .as_mut()
                    .and_then(|v| crate::app::read_note(v, &p).ok())
            }),
            None => None,
        }
    };
    let text = text.unwrap_or_default();
    let mut items = anchor_items(note, rest, &text);
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
pub fn anchor_items(note: &str, typed: &str, text: &str) -> Vec<(String, String)> {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut out: Vec<(i32, String, String)> = Vec::new();
    if let Some(want) = typed.strip_prefix('^') {
        for (id, line) in crate::blocks::block_ids(&lines) {
            let score = if want.is_empty() {
                Some(0)
            } else {
                crate::text::fuzzy(&lower(want), &id)
                    .or_else(|| crate::text::fuzzy(&lower(want), &line))
            };
            if let Some(s) = score {
                let mut ins = String::from(note);
                ins.push_str("#^");
                ins.push_str(&id);
                let mut label: String = line.chars().take(40).collect::<String>().replace('/', " ");
                label.push('/');
                out.push((s, ins, label));
            }
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
        let h = anchor_items("Note", "", text);
        assert_eq!(
            h,
            [
                (String::from("Note#One"), String::from("H1/")),
                (String::from("Note#Two things"), String::from("H2/")),
            ]
        );
        let t = anchor_items("Note", "thi", text);
        assert_eq!(t[0].0, "Note#Two things");
        let b = anchor_items("Note", "^", text);
        assert_eq!(b.len(), 2);
        assert_eq!(b[0].0, "Note#^p1");
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
