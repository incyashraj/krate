//! CSV, both ways (RFC 4180, plus what spreadsheets actually save).

use super::{number_exact, Cell, Sheet};
use alloc::{string::String, vec::Vec};

/// Read CSV text into a sheet named "Sheet1".
///
/// The separator is whichever of `,` `;` or tab the first line uses most
/// (Excel in much of Europe saves `;`). Quoted fields may hold separators,
/// line breaks and doubled quotes. A byte-order mark is dropped. Every
/// filled cell is [`Cell::Text`] -- CSV has no types -- and an empty field
/// is [`Cell::Empty`]; use [`Cell::as_number`] for numbers.
pub fn read_csv(text: &str) -> Sheet {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let sep = detect_separator(text);
    let mut sheet = Sheet::new("Sheet1");
    let mut row: Vec<Cell> = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut was_quoted = false;
    let mut chars = text.chars().peekable();
    let push_field = |row: &mut Vec<Cell>, field: &mut String, was_quoted: bool| {
        if field.is_empty() && !was_quoted {
            row.push(Cell::Empty);
        } else {
            row.push(Cell::Text(core::mem::take(field)));
        }
    };
    let mut row_started = false;
    while let Some(ch) = chars.next() {
        row_started = true;
        if quoted {
            if ch == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    quoted = false;
                }
            } else {
                field.push(ch);
            }
            continue;
        }
        match ch {
            '"' if field.is_empty() && !was_quoted => {
                quoted = true;
                was_quoted = true;
            }
            c if c == sep => {
                push_field(&mut row, &mut field, was_quoted);
                was_quoted = false;
            }
            '\r' | '\n' => {
                if ch == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                }
                push_field(&mut row, &mut field, was_quoted);
                was_quoted = false;
                end_row(&mut sheet, &mut row);
                row_started = false;
            }
            c => field.push(c),
        }
    }
    if row_started {
        push_field(&mut row, &mut field, was_quoted);
        end_row(&mut sheet, &mut row);
    }
    // A sheet has no trailing blank rows.
    while sheet.rows.last().is_some_and(|r| r.is_empty()) {
        sheet.rows.pop();
    }
    sheet
}

fn end_row(sheet: &mut Sheet, row: &mut Vec<Cell>) {
    while matches!(row.last(), Some(Cell::Empty)) {
        row.pop();
    }
    sheet.rows.push(core::mem::take(row));
}

fn detect_separator(text: &str) -> char {
    let mut counts = [0usize; 3];
    let mut quoted = false;
    for ch in text.chars() {
        match ch {
            '"' => quoted = !quoted,
            '\n' | '\r' if !quoted => break,
            ',' if !quoted => counts[0] += 1,
            ';' if !quoted => counts[1] += 1,
            '\t' if !quoted => counts[2] += 1,
            _ => {}
        }
    }
    let seps = [',', ';', '\t'];
    let mut best = 0;
    for i in 1..3 {
        if counts[i] > counts[best] {
            best = i;
        }
    }
    seps[best]
}

/// Write a sheet as CSV.
///
/// Comma-separated, CRLF line ends, starting with a UTF-8 byte-order mark
/// (without it Excel reads accented letters and other scripts as
/// gibberish). Fields holding a comma, a quote, a line break or edge spaces
/// are quoted. Numbers are written exactly, dates as ISO 8601
/// (`2024-03-15`, `2024-03-15T09:30:00`), TRUE/FALSE as words.
pub fn write_csv(sheet: &Sheet) -> String {
    let mut out = String::from("\u{feff}");
    // Every row as wide as the widest, the way spreadsheets export: a
    // reader that expects a rectangle (pandas, a database import) refuses
    // ragged rows.
    let width = sheet.width();
    for row in &sheet.rows {
        for (i, cell) in row.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            let text = match cell {
                Cell::Empty => String::new(),
                Cell::Number(n) => number_exact(*n),
                Cell::Date(d) => d.to_iso(),
                other => other.to_text(),
            };
            let needs_quotes = text.contains([',', '"', '\n', '\r'])
                || text.starts_with([' ', '\t'])
                || text.ends_with([' ', '\t']);
            if needs_quotes {
                out.push('"');
                for ch in text.chars() {
                    if ch == '"' {
                        out.push('"');
                    }
                    out.push(ch);
                }
                out.push('"');
            } else {
                out.push_str(&text);
            }
        }
        for _ in row.len().max(1)..width {
            out.push(',');
        }
        out.push_str("\r\n");
    }
    out
}

/// Windows-1252, the code page Excel on Windows saves CSV in. Bytes
/// 0x80-0x9F are the curly quotes, the euro sign and friends; the rest is
/// Latin-1.
pub(crate) fn decode_1252(bytes: &[u8]) -> String {
    const HIGH: [char; 32] = [
        '\u{20ac}', '\u{81}', '\u{201a}', '\u{192}', '\u{201e}', '\u{2026}', '\u{2020}',
        '\u{2021}', '\u{2c6}', '\u{2030}', '\u{160}', '\u{2039}', '\u{152}', '\u{8d}', '\u{17d}',
        '\u{8f}', '\u{90}', '\u{2018}', '\u{2019}', '\u{201c}', '\u{201d}', '\u{2022}', '\u{2013}',
        '\u{2014}', '\u{2dc}', '\u{2122}', '\u{161}', '\u{203a}', '\u{153}', '\u{9d}', '\u{17e}',
        '\u{178}',
    ];
    bytes
        .iter()
        .map(|&b| match b {
            0x80..=0x9f => HIGH.get(usize::from(b - 0x80)).copied().unwrap_or('?'),
            b => char::from(b),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn texts(sheet: &Sheet) -> Vec<Vec<String>> {
        sheet
            .rows
            .iter()
            .map(|r| r.iter().map(Cell::to_text).collect())
            .collect()
    }

    #[test]
    fn quotes_separators_and_line_breaks() {
        let s = read_csv("\u{feff}a,\"b,1\",\"say \"\"hi\"\"\"\r\n\"two\nlines\",,x\n\n");
        assert_eq!(
            texts(&s),
            vec![vec!["a", "b,1", "say \"hi\""], vec!["two\nlines", "", "x"]]
        );
        assert_eq!(s.get(1, 1), &Cell::Empty);
    }

    #[test]
    fn semicolons_and_tabs_are_detected() {
        assert_eq!(
            texts(&read_csv("a;b;c\n1,5;2;3")),
            vec![vec!["a", "b", "c"], vec!["1,5", "2", "3"]]
        );
        assert_eq!(
            texts(&read_csv("a\tb\n1\t2")),
            vec![vec!["a", "b"], vec!["1", "2"]]
        );
    }

    #[test]
    fn writing_then_reading_keeps_every_field() {
        let mut sheet = Sheet::new("x");
        sheet.push_row(vec![
            Cell::from("plain"),
            Cell::from("a,b"),
            Cell::from("q\"q"),
            Cell::from(" edge "),
            Cell::from("line\nbreak"),
            Cell::from("Zürich 東京 🚀"),
        ]);
        sheet.push_row(vec![Cell::from(0.1), Cell::Empty, Cell::from(true)]);
        sheet.push_row(Vec::new());
        let csv = write_csv(&sheet);
        assert!(
            csv.ends_with("0.1,,TRUE,,,\r\n,,,,,\r\n"),
            "rows padded to a rectangle"
        );
        let back = read_csv(&csv);
        assert_eq!(texts(&back)[0], texts(&sheet)[0]);
        assert_eq!(texts(&back)[1], vec!["0.1", "", "TRUE"]);
    }

    #[test]
    fn windows_1252_decodes() {
        assert_eq!(
            decode_1252(b"Z\xfcrich \x80 \x93q\x94"),
            "Zürich € \u{201c}q\u{201d}"
        );
    }
}
