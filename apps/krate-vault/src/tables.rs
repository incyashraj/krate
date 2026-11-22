//! Editing a table inside the drawn table, as Obsidian 1.15's live preview
//! does: the caret sits in a cell, Tab and Shift+Tab go cell to cell (Tab
//! past the last cell adds a row), Enter goes down a row (and past the
//! last adds one), Up and Down keep the column, a typed `|` is escaped,
//! and the table menu adds, deletes, moves and aligns rows and columns.
//!
//! The note's text stays the source: every change is an edit of the
//! table's lines through `Doc::replace`, one undo each.

use crate::blocks::{self, Align};
use crate::buffer::Pos;
use crate::editor::{Doc, SpanKind};
use alloc::string::String;
use alloc::vec::Vec;

/// What the table menu can do to the cell the caret is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    RowAbove,
    RowBelow,
    DeleteRow,
    DuplicateRow,
    MoveRowUp,
    MoveRowDown,
    ColumnLeft,
    ColumnRight,
    DeleteColumn,
    DuplicateColumn,
    MoveColumnLeft,
    MoveColumnRight,
    Align(Align),
    SortAscending,
    SortDescending,
}

/// The menu Obsidian shows on a table cell, in its order and words.
pub fn menu() -> Vec<(&'static str, Op)> {
    alloc::vec![
        ("Add row above", Op::RowAbove),
        ("Add row below", Op::RowBelow),
        ("Move row up", Op::MoveRowUp),
        ("Move row down", Op::MoveRowDown),
        ("Duplicate row", Op::DuplicateRow),
        ("Delete row", Op::DeleteRow),
        ("Add column before", Op::ColumnLeft),
        ("Add column after", Op::ColumnRight),
        ("Move column left", Op::MoveColumnLeft),
        ("Move column right", Op::MoveColumnRight),
        ("Duplicate column", Op::DuplicateColumn),
        ("Delete column", Op::DeleteColumn),
        ("Align left", Op::Align(Align::Left)),
        ("Align center", Op::Align(Align::Center)),
        ("Align right", Op::Align(Align::Right)),
        ("Sort by column, A to Z", Op::SortAscending),
        ("Sort by column, Z to A", Op::SortDescending),
    ]
}

/// The table the caret is in: its first and last line.
pub fn table_at(doc: &Doc, line: usize) -> Option<(usize, usize)> {
    let s = doc.span(line)?;
    (s.kind == SpanKind::Table).then_some((s.first, s.last))
}

/// Which cell of its row the caret is in, by byte; cells as split_row
/// gives them (trimmed).
pub fn cell_index(line: &str, col: usize) -> usize {
    let cells = blocks::split_row(line);
    let mut best = 0;
    for (i, (a, _)) in cells.iter().enumerate() {
        // A cell owns everything from the pipe before it.
        let pipe = line.get(..*a).and_then(|h| h.rfind('|')).unwrap_or(0);
        if col > pipe || (i == 0 && col <= *a) {
            best = i;
        }
    }
    best
}

fn cell_range(line: &str, i: usize) -> Option<(usize, usize)> {
    let (a, z) = blocks::split_row(line).get(i).copied()?;
    if a == z {
        // An empty cell: the caret goes one space after its pipe.
        let pipe = line.get(..a).and_then(|h| h.rfind('|'));
        if let Some(p) = pipe {
            let at = (p + 2).min(a.max(p + 1));
            return Some((at, at));
        }
    }
    Some((a, z))
}

/// Put the caret at the end of cell `i` of `line` (or its start).
pub fn to_cell(doc: &mut Doc, line: usize, i: usize, at_end: bool) {
    let text = String::from(doc.line(line));
    let (a, z) = cell_range(&text, i).unwrap_or((text.len(), text.len()));
    doc.place(Pos::new(line, if at_end { z } else { a }), false);
}

/// The table's rows as cell texts (the delimiter row left out) and the
/// column alignments.
fn read(doc: &Doc, first: usize, last: usize) -> (Vec<Vec<String>>, Vec<Align>) {
    let aligns = blocks::delimiter_row(doc.line(first + 1)).unwrap_or_default();
    let mut rows = Vec::new();
    for n in first..=last {
        if n == first + 1 {
            continue;
        }
        let l = doc.line(n);
        let mut cells: Vec<String> = blocks::split_row(l)
            .iter()
            .map(|(a, z)| String::from(l.get(*a..*z).unwrap_or("")))
            .collect();
        cells.resize(aligns.len().max(cells.len()), String::new());
        rows.push(cells);
    }
    (rows, aligns)
}

fn write_row(cells: &[String]) -> String {
    let mut s = String::from("|");
    for c in cells {
        s.push(' ');
        s.push_str(c);
        s.push_str(" |");
    }
    s
}

fn delimiter(aligns: &[Align]) -> String {
    let mut s = String::from("|");
    for a in aligns {
        s.push(' ');
        s.push_str(match a {
            Align::Left => ":---",
            Align::Center => ":---:",
            Align::Right => "---:",
            Align::Default => "---",
        });
        s.push_str(" |");
    }
    s
}

fn write_table(
    doc: &mut Doc,
    first: usize,
    last: usize,
    rows: &[Vec<String>],
    aligns: &[Align],
    now: u64,
) {
    let mut text = String::new();
    for (i, r) in rows.iter().enumerate() {
        if i > 0 {
            text.push('\n');
        }
        text.push_str(&write_row(r));
        if i == 0 {
            text.push('\n');
            text.push_str(&delimiter(aligns));
        }
    }
    let end = Pos::new(last, doc.buf.line_len(last));
    doc.buf.begin_group(doc.cur);
    doc.replace(Pos::new(first, 0), end, &text, now);
    doc.buf.end_group(doc.cur);
}

/// The note line of table row `r` (0 is the header).
fn line_of(first: usize, r: usize) -> usize {
    if r == 0 {
        first
    } else {
        first + 1 + r
    }
}

/// Do a menu action at the caret's cell. False when the caret is not in a
/// table.
pub fn apply(doc: &mut Doc, op: Op, now: u64) -> bool {
    let Some((first, last)) = table_at(doc, doc.cur.line) else {
        return false;
    };
    let (mut rows, mut aligns) = read(doc, first, last);
    let ncols = aligns.len().max(1);
    let r = if doc.cur.line == first {
        0
    } else {
        doc.cur.line.saturating_sub(first + 1).max(1)
    };
    let c = cell_index(doc.line(doc.cur.line), doc.cur.col).min(ncols - 1);
    let blank = || alloc::vec![String::new(); ncols];
    let (mut tr, mut tc) = (r, c);
    match op {
        Op::RowAbove => {
            let at = r.max(1);
            rows.insert(at, blank());
            tr = at;
        }
        Op::RowBelow => {
            rows.insert(r + 1, blank());
            tr = r + 1;
        }
        Op::DeleteRow => {
            if r == 0 || rows.len() <= 2 {
                // The header stays; a table keeps one body row.
                if let Some(row) = rows.get_mut(r) {
                    row.iter_mut().for_each(String::clear);
                }
            } else {
                rows.remove(r);
                tr = r.min(rows.len() - 1);
            }
        }
        Op::DuplicateRow => {
            let copy = rows.get(r).cloned().unwrap_or_else(blank);
            rows.insert(r.max(1) + usize::from(r > 0), copy);
            tr = r + 1;
        }
        Op::MoveRowUp if r > 1 => {
            rows.swap(r, r - 1);
            tr = r - 1;
        }
        Op::MoveRowDown if r > 0 && r + 1 < rows.len() => {
            rows.swap(r, r + 1);
            tr = r + 1;
        }
        Op::ColumnLeft | Op::ColumnRight => {
            let at = if op == Op::ColumnLeft { c } else { c + 1 };
            for row in rows.iter_mut() {
                row.insert(at.min(row.len()), String::new());
            }
            aligns.insert(at.min(aligns.len()), Align::Default);
            tc = at;
        }
        Op::DeleteColumn => {
            if ncols > 1 {
                for row in rows.iter_mut() {
                    if c < row.len() {
                        row.remove(c);
                    }
                }
                aligns.remove(c);
                tc = c.min(ncols - 2);
            }
        }
        Op::DuplicateColumn => {
            for row in rows.iter_mut() {
                let v = row.get(c).cloned().unwrap_or_default();
                row.insert(c + 1, v);
            }
            let a = aligns.get(c).copied().unwrap_or(Align::Default);
            aligns.insert(c + 1, a);
            tc = c + 1;
        }
        Op::MoveColumnLeft if c > 0 => {
            for row in rows.iter_mut() {
                row.swap(c, c - 1);
            }
            aligns.swap(c, c - 1);
            tc = c - 1;
        }
        Op::MoveColumnRight if c + 1 < ncols => {
            for row in rows.iter_mut() {
                row.swap(c, c + 1);
            }
            aligns.swap(c, c + 1);
            tc = c + 1;
        }
        Op::Align(a) => {
            if let Some(slot) = aligns.get_mut(c) {
                *slot = a;
            }
        }
        Op::SortAscending | Op::SortDescending => {
            let key = |row: &Vec<String>| crate::text::lower(row.get(c).map_or("", String::as_str));
            let body = rows.get_mut(1..).unwrap_or(&mut []);
            body.sort_by_key(key);
            if op == Op::SortDescending {
                body.reverse();
            }
        }
        _ => return true,
    }
    write_table(doc, first, last, &rows, &aligns, now);
    to_cell(doc, line_of(first, tr), tc, true);
    true
}

/// Keep the caret inside a cell: one that arrives on a table row outside
/// every cell (on a pipe, before the first) goes to the nearest cell.
pub fn snap(doc: &mut Doc) {
    let Some((first, _)) = table_at(doc, doc.cur.line) else {
        return;
    };
    if doc.cur.line == first + 1 {
        to_cell(doc, first + 2, 0, false);
        return;
    }
    let line = String::from(doc.line(doc.cur.line));
    let c = cell_index(&line, doc.cur.col);
    if let Some((a, z)) = cell_range(&line, c) {
        if doc.cur.col < a || doc.cur.col > z {
            let col = doc.cur.col.clamp(a, z);
            let anchor = doc.anchor;
            doc.cur = Pos::new(doc.cur.line, col);
            doc.anchor = anchor;
        }
    }
}

/// What a key does with the caret in a table: `None` when the key is not
/// a table key (the editor handles it as usual).
pub fn key(doc: &mut Doc, key: &str, shift: bool, now: u64) -> Option<()> {
    let (first, last) = table_at(doc, doc.cur.line)?;
    snap(doc);
    let line = String::from(doc.line(doc.cur.line));
    let ncols = blocks::split_row(&line).len().max(1);
    let c = cell_index(&line, doc.cur.col).min(ncols - 1);
    let row_line = doc.cur.line;
    let next_line = |l: usize| if l == first { first + 2 } else { l + 1 };
    let prev_line = |l: usize| {
        if l == first + 2 {
            first
        } else {
            l.saturating_sub(1)
        }
    };
    match key {
        "Tab" if !shift => {
            if c + 1 < ncols {
                to_cell(doc, row_line, c + 1, true);
            } else if row_line < last {
                to_cell(doc, next_line(row_line), 0, true);
            } else {
                apply(doc, Op::RowBelow, now);
                to_cell(doc, last + 1, 0, true);
            }
        }
        "Tab" => {
            if c > 0 {
                to_cell(doc, row_line, c - 1, true);
            } else if row_line > first {
                to_cell(doc, prev_line(row_line), ncols - 1, true);
            }
        }
        "Enter" if !shift => {
            if row_line < last {
                to_cell(doc, next_line(row_line), c, true);
            } else {
                apply(doc, Op::RowBelow, now);
                to_cell(doc, last + 1, c, true);
            }
        }
        "ArrowDown" => {
            if row_line < last {
                to_cell(doc, next_line(row_line), c, true);
            } else {
                // Off the bottom: the line after the table.
                let p = Pos::new((last + 1).min(doc.buf.last_line()), 0);
                if last == doc.buf.last_line() {
                    return None;
                }
                doc.place(p, false);
            }
        }
        "ArrowUp" => {
            if row_line > first {
                to_cell(doc, prev_line(row_line), c, true);
            } else if first > 0 {
                let p = Pos::new(first - 1, doc.buf.line_len(first - 1));
                doc.place(p, false);
            }
        }
        "ArrowLeft" | "ArrowRight" | "Home" | "End" | "Backspace" | "Delete" => {
            let (a, z) = cell_range(&line, c).unwrap_or((doc.cur.col, doc.cur.col));
            let col = doc.cur.col.clamp(a, z);
            match key {
                "ArrowLeft" if col <= a && doc.selection().is_none() => {
                    if c > 0 {
                        to_cell(doc, row_line, c - 1, true);
                    } else if row_line > first {
                        to_cell(doc, prev_line(row_line), ncols - 1, true);
                    }
                }
                "ArrowRight" if col >= z && doc.selection().is_none() => {
                    if c + 1 < ncols {
                        to_cell(doc, row_line, c + 1, false);
                    } else if row_line < last {
                        to_cell(doc, next_line(row_line), 0, false);
                    }
                }
                "Home" => doc.place(Pos::new(row_line, a), shift),
                "End" => doc.place(Pos::new(row_line, z), shift),
                // Deleting stops at the cell's edges: a pipe is never eaten.
                "Backspace" if col <= a && doc.selection().is_none() => {}
                "Delete" if col >= z && doc.selection().is_none() => {}
                _ => return None,
            }
        }
        _ => return None,
    }
    Some(())
}

/// Typed text in a table cell: a `|` is escaped so it stays in the cell.
pub fn typed(doc: &mut Doc, s: &str) -> Option<String> {
    table_at(doc, doc.cur.line)?;
    snap(doc);
    s.contains('|').then(|| s.replace('|', "\\|"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(t: &str) -> Doc {
        let mut d = Doc::new("n.md", "n", t);
        d.fresh = false;
        d
    }

    const T: &str = "# T\n\n| a | b |\n| --- | :-: |\n| 1 | 2 |\n| 3 | 4 |\n\nafter";

    #[test]
    fn tab_enter_and_arrows_go_cell_to_cell() {
        let mut d = doc(T);
        d.place(Pos::new(4, 3), false); // in "1"
        key(&mut d, "Tab", false, 0).unwrap();
        assert_eq!(d.cur, Pos::new(4, 7), "end of the 2");
        key(&mut d, "Tab", false, 0).unwrap();
        assert_eq!(d.cur, Pos::new(5, 3), "the next row's first cell");
        key(&mut d, "Tab", true, 0).unwrap();
        assert_eq!(d.cur, Pos::new(4, 7));
        key(&mut d, "ArrowUp", false, 0).unwrap();
        assert_eq!(d.cur, Pos::new(2, 7), "up skips the delimiter row");
        key(&mut d, "Enter", false, 0).unwrap();
        assert_eq!(d.cur.line, 4);
        // Tab past the last cell adds a row.
        d.place(Pos::new(5, 7), false);
        key(&mut d, "Tab", false, 0).unwrap();
        assert_eq!(d.line(6), "|  |  |");
        assert_eq!(d.cur, Pos::new(6, 2), "in the new row's first cell");
        assert_eq!(d.line(7), "");
        d.undo();
        assert_eq!(d.line(6), "", "one undo takes the row back");
        // A typed pipe stays in the cell.
        assert_eq!(typed(&mut d, "a|b").as_deref(), Some("a\\|b"));
        // A caret that lands on a pipe moves into the cell.
        d.place(Pos::new(4, 0), false);
        snap(&mut d);
        assert_eq!(d.cur, Pos::new(4, 2));
        assert!(key(&mut d, "x", false, 0).is_none());
    }

    #[test]
    fn the_menu_reshapes_the_table() {
        let mut d = doc(T);
        d.place(Pos::new(4, 3), false);
        apply(&mut d, Op::RowBelow, 0);
        assert_eq!(d.line(5), "|  |  |");
        assert_eq!(d.cur.line, 5);
        apply(&mut d, Op::DeleteRow, 0);
        assert_eq!(d.line(5), "| 3 | 4 |");
        d.place(Pos::new(4, 3), false);
        apply(&mut d, Op::ColumnRight, 0);
        assert_eq!(d.line(2), "| a |  | b |");
        assert_eq!(d.line(3), "| --- | --- | :---: |");
        apply(&mut d, Op::DeleteColumn, 0);
        assert_eq!(d.line(2), "| a | b |");
        d.place(Pos::new(4, 3), false);
        apply(&mut d, Op::Align(Align::Right), 0);
        assert_eq!(d.line(3), "| ---: | :---: |");
        apply(&mut d, Op::MoveColumnRight, 0);
        assert_eq!(d.line(4), "| 2 | 1 |");
        apply(&mut d, Op::SortDescending, 0);
        assert_eq!(d.line(4), "| 4 | 3 |");
        apply(&mut d, Op::MoveRowDown, 0);
        assert_eq!(d.line(5), "| 4 | 3 |");
        assert_eq!(d.line(6), "", "the blank line after the table stays");
        assert_eq!(d.line(7), "after");
        assert_eq!(menu().len(), 17);
    }
}
