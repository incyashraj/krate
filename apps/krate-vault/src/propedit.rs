//! Editing properties inside the Properties block, as Obsidian does: a
//! value (or a key) becomes a field where it is drawn, list values are
//! chips with a cross to take one away and a field to add one, a date
//! opens a month to pick from, a checkbox ticks, and the type menu turns a
//! property into another kind. Every change is written back to the note's
//! YAML lines, one undo each.

use crate::app::{Input, Mods};
use crate::buffer::Pos;
use crate::editor::Doc;
use crate::props::{self, Kind, Prop, Value};
use alloc::string::String;
use alloc::vec::Vec;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Key,
    Value,
    /// The field after a list's chips, adding an item.
    NewItem,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PropEdit {
    /// The note line of the property's key (a new one is written first).
    pub line: usize,
    pub field: Field,
    pub input: Input,
    /// The month the date picker shows, when it is open.
    pub picker: Option<(i32, u32)>,
}

/// The note's properties.
pub fn props(doc: &Doc) -> Vec<Prop> {
    let Some(end) = doc.front else {
        return Vec::new();
    };
    let lines: Vec<&str> = (1..end).map(|n| doc.line(n)).collect();
    let mut out = props::parse(&lines, 1);
    props::apply_types(&mut out, &doc.prop_types);
    out
}

fn prop_at(doc: &Doc, line: usize) -> Option<Prop> {
    props(doc).into_iter().find(|p| p.line == line)
}

/// Start editing a property's key or value (or adding to its list).
pub fn begin(doc: &mut Doc, line: usize, field: Field) {
    let Some(p) = prop_at(doc, line) else {
        return;
    };
    let text = match field {
        Field::Key => p.key.clone(),
        Field::Value => match &p.value {
            Value::List(_) => String::new(),
            v => v.shown_raw(),
        },
        Field::NewItem => String::new(),
    };
    let field = match (&p.value, field) {
        (Value::List(_), Field::Value) => Field::NewItem,
        (_, f) => f,
    };
    let mut input = Input::with(&text);
    input.select_all();
    let picker = if field == Field::Value && matches!(p.kind, Kind::Date | Kind::DateTime) {
        let d = p.value.shown_raw();
        let y = d.get(..4).and_then(|x| x.parse().ok()).unwrap_or(2026);
        let m = d.get(5..7).and_then(|x| x.parse().ok()).unwrap_or(1);
        Some((y, m))
    } else {
        None
    };
    doc.prop_edit = Some(PropEdit {
        line,
        field,
        input,
        picker,
    });
    doc.fresh = false;
}

fn write(doc: &mut Doc, p: &Prop, key: &str, kind: Kind, value: &Value, now: u64) -> usize {
    let text = props::render(key, kind, value);
    let end = Pos::new(p.last, doc.buf.line_len(p.last));
    let (cur, anchor) = (doc.cur, doc.anchor);
    doc.buf.begin_group(cur);
    doc.replace(Pos::new(p.line, 0), end, &text, now);
    doc.buf.end_group(cur);
    // The caret stays where it was in the body, shifted by the lines the
    // property gained or lost.
    let delta = text.matches('\n').count() as isize - (p.last - p.line) as isize;
    if cur.line > p.last {
        doc.cur = doc
            .buf
            .clamp(Pos::new((cur.line as isize + delta) as usize, cur.col));
        doc.anchor = anchor.map(|a| {
            doc.buf
                .clamp(Pos::new((a.line as isize + delta).max(0) as usize, a.col))
        });
    } else {
        doc.cur = doc.buf.clamp(cur);
        doc.anchor = None;
    }
    p.line
}

/// Write what the field holds and stop editing it (`keep_adding` keeps a
/// list's add field open for the next item).
pub fn commit(doc: &mut Doc, now: u64, keep_adding: bool) {
    let Some(e) = doc.prop_edit.take() else {
        return;
    };
    let Some(p) = prop_at(doc, e.line) else {
        return;
    };
    let typed = e.input.text.trim();
    match e.field {
        Field::Key => {
            if !typed.is_empty() && typed != p.key {
                write(doc, &p, typed, p.kind, &p.value, now);
            }
        }
        Field::Value => {
            let v = props::from_input(p.kind, typed);
            if v != p.value {
                write(doc, &p, &p.key, p.kind, &v, now);
            }
        }
        Field::NewItem => {
            if !typed.is_empty() {
                let mut items = match &p.value {
                    Value::List(v) => v.clone(),
                    Value::Empty => Vec::new(),
                    other => alloc::vec![other.shown_raw()],
                };
                for part in typed.split(',') {
                    let part = part.trim();
                    if !part.is_empty() && !items.iter().any(|i| i == part) {
                        items.push(String::from(part));
                    }
                }
                let kind = if matches!(p.kind, Kind::Tags | Kind::Aliases) {
                    p.kind
                } else {
                    Kind::List
                };
                write(doc, &p, &p.key, kind, &Value::List(items), now);
            }
            if keep_adding {
                doc.prop_edit = Some(PropEdit {
                    line: e.line,
                    field: Field::NewItem,
                    input: Input::default(),
                    picker: None,
                });
            }
        }
    }
}

/// Take item `i` out of a list property (the chip's cross).
pub fn remove_item(doc: &mut Doc, line: usize, i: usize, now: u64) {
    let Some(p) = prop_at(doc, line) else {
        return;
    };
    if let Value::List(mut items) = p.value.clone() {
        if i < items.len() {
            items.remove(i);
            write(doc, &p, &p.key, p.kind, &Value::List(items), now);
        }
    }
}

/// Hand every open note the types the vault keeps for property names
/// (`.obsidian/types.json`), so a type picked in one note shows in all.
pub fn share_types(app: &mut crate::app::App) {
    let types = props::types_from(&app.settings.types.data);
    let tabs = app
        .tabs
        .iter_mut()
        .chain(app.panes.iter_mut().flat_map(|p| p.tabs.iter_mut()));
    for t in tabs {
        if let Some(d) = t.doc.as_mut() {
            if d.prop_types != types {
                d.prop_types = types.clone();
            }
        }
    }
}

/// The name of the property whose key is on `line`.
pub fn key_at(doc: &Doc, line: usize) -> Option<String> {
    prop_at(doc, line).map(|p| p.key)
}

/// The type menu: turn a property into another kind.
pub fn set_kind(doc: &mut Doc, line: usize, kind: Kind, now: u64) {
    let Some(p) = prop_at(doc, line) else {
        return;
    };
    let v = props::convert(&p.value, kind);
    write(doc, &p, &p.key, kind, &v, now);
}

/// Remove a property (the type menu's Remove).
pub fn remove(doc: &mut Doc, line: usize, now: u64) {
    let Some(p) = prop_at(doc, line) else {
        return;
    };
    let end = if p.last + 1 < doc.buf.line_count() {
        Pos::new(p.last + 1, 0)
    } else {
        Pos::new(p.last, doc.buf.line_len(p.last))
    };
    doc.replace(Pos::new(p.line, 0), end, "", now);
    doc.prop_edit = None;
}

/// A day picked in the date picker.
pub fn pick_date(doc: &mut Doc, line: usize, y: i32, m: u32, d: u32, now: u64) {
    let Some(p) = prop_at(doc, line) else {
        return;
    };
    let mut s = String::new();
    let pad = |s: &mut String, v: u32, w: usize| {
        let t = alloc::format!("{v:0w$}");
        s.push_str(&t);
    };
    pad(&mut s, y.max(0) as u32, 4);
    s.push('-');
    pad(&mut s, m, 2);
    s.push('-');
    pad(&mut s, d, 2);
    if p.kind == Kind::DateTime {
        let old = p.value.shown_raw();
        s.push_str(old.get(10..).filter(|t| !t.is_empty()).unwrap_or("T00:00"));
    }
    doc.prop_edit = None;
    write(doc, &p, &p.key, p.kind, &Value::Date(s), now);
}

/// Move the picker a month back or on.
pub fn turn_month(doc: &mut Doc, forward: bool) {
    if let Some((y, m)) = doc.prop_edit.as_mut().and_then(|e| e.picker.as_mut()) {
        if forward {
            if *m == 12 {
                *m = 1;
                *y += 1;
            } else {
                *m += 1;
            }
        } else if *m == 1 {
            *m = 12;
            *y -= 1;
        } else {
            *m -= 1;
        }
    }
}

/// Keys while a field is open. False lets the editor have the key.
pub fn key(doc: &mut Doc, key: &str, m: Mods, now: u64) -> bool {
    let Some(e) = doc.prop_edit.as_mut() else {
        return false;
    };
    match key {
        "Enter" => {
            let adding = e.field == Field::NewItem && !e.input.text.trim().is_empty();
            commit(doc, now, adding);
        }
        "Escape" => commit(doc, now, false),
        "Tab" if e.field == Field::Key && !m.shift => {
            // From a name to its own value.
            let line = e.line;
            commit(doc, now, false);
            begin(doc, line, Field::Value);
        }
        "Tab" => {
            // On to the next property's value (Shift: the one before).
            let line = e.line;
            commit(doc, now, false);
            let all = props(doc);
            let i = all.iter().position(|p| p.line >= line).unwrap_or(0);
            let next = if m.shift {
                i.checked_sub(1)
            } else {
                Some(i + 1)
            };
            if let Some(p) = next.and_then(|j| all.get(j)) {
                let l = p.line;
                begin(doc, l, Field::Value);
            }
        }
        "Backspace" if e.field == Field::NewItem && e.input.text.is_empty() => {
            // An empty add field takes away the last chip.
            let line = e.line;
            if let Some(Value::List(items)) = prop_at(doc, line).map(|p| p.value) {
                if !items.is_empty() {
                    remove_item(doc, line, items.len() - 1, now);
                }
            }
            if let Some(e) = doc.prop_edit.as_mut() {
                e.line = line;
            }
        }
        _ => {
            if m.primary && key.eq_ignore_ascii_case("a") {
                e.input.select_all();
            } else {
                e.input.key(key, m);
            }
        }
    }
    true
}

/// Typing while a field is open.
pub fn text(doc: &mut Doc, s: &str) -> bool {
    let Some(e) = doc.prop_edit.as_mut() else {
        return false;
    };
    if s == "\n" {
        return false;
    }
    e.input.insert(s);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOTE: &str = "---\ntitle: Render test\ntags:\n  - render\n  - test\nrating: 4\ncreated: 2026-11-22\ndone: false\n---\nBody";

    fn doc() -> Doc {
        let mut d = Doc::new("n.md", "n", NOTE);
        d.fresh = false;
        d
    }

    fn typed(d: &mut Doc, s: &str) {
        text(d, s);
    }

    #[test]
    fn values_and_keys_are_edited_in_place() {
        let mut d = doc();
        let body = d.cur;
        begin(&mut d, 1, Field::Value);
        assert_eq!(d.prop_edit.as_ref().unwrap().input.text, "Render test");
        typed(&mut d, "New title: yes");
        key(&mut d, "Enter", Mods::default(), 0);
        assert_eq!(d.line(1), "title: \"New title: yes\"");
        assert_eq!(d.cur, body, "the caret stays in the body");
        begin(&mut d, 1, Field::Key);
        typed(&mut d, "name");
        key(&mut d, "Escape", Mods::default(), 0);
        assert!(d.line(1).starts_with("name: "));
        d.undo();
        assert!(d.line(1).starts_with("title: "));
    }

    #[test]
    fn list_chips_add_and_remove() {
        let mut d = doc();
        begin(&mut d, 2, Field::Value);
        assert_eq!(d.prop_edit.as_ref().unwrap().field, Field::NewItem);
        typed(&mut d, "draft");
        key(&mut d, "Enter", Mods::default(), 0);
        assert_eq!(d.line(5), "  - draft");
        assert_eq!(
            d.prop_edit.as_ref().unwrap().field,
            Field::NewItem,
            "ready for the next"
        );
        key(&mut d, "Backspace", Mods::default(), 0);
        assert_eq!(d.line(5), "rating: 4");
        remove_item(&mut d, 2, 0, 0);
        assert_eq!((d.line(3), d.line(4)), ("  - test", "rating: 4"));
        assert_eq!(d.line(8), "Body");
    }

    #[test]
    fn types_dates_and_removal() {
        let mut d = doc();
        set_kind(&mut d, 5, Kind::Text, 0);
        assert_eq!(d.line(5), "rating: \"4\"");
        set_kind(&mut d, 5, Kind::Number, 0);
        assert_eq!(d.line(5), "rating: 4");
        set_kind(&mut d, 5, Kind::List, 0);
        assert_eq!((d.line(5), d.line(6)), ("rating:", "  - \"4\""));
        begin(&mut d, 7, Field::Value);
        assert_eq!(d.prop_edit.as_ref().unwrap().picker, Some((2026, 11)));
        turn_month(&mut d, true);
        assert_eq!(d.prop_edit.as_ref().unwrap().picker, Some((2026, 12)));
        pick_date(&mut d, 7, 2026, 12, 3, 0);
        assert_eq!(d.line(7), "created: 2026-12-03");
        remove(&mut d, 7, 0);
        assert_eq!(d.line(7), "done: false");
        // Tab walks to the next property's value.
        begin(&mut d, 1, Field::Value);
        key(&mut d, "Tab", Mods::default(), 0);
        assert_eq!(d.prop_edit.as_ref().unwrap().line, 2);
    }
}
