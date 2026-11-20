//! OpenDocument spreadsheets (.ods), as LibreOffice and Google Sheets save.

use super::xml::{self, attr, attr_text, local, push_unescaped, Event};
use super::zip::Archive;
use super::{Cell, DateTime, Sheet, SheetError, Workbook};
use alloc::{string::String, vec::Vec};

/// LibreOffice pads a sheet to its full size with one row or cell
/// "repeated" a million times. Repeats of an empty cell only move the
/// position; a filled cell or row is copied at most this many times.
const MAX_FILLED_REPEAT: usize = 1024;
const MAX_ROWS: usize = 1_048_576;
const MAX_COLS: usize = 16_384;

pub(crate) fn looks_like(zip: &Archive<'_>) -> bool {
    zip.has("content.xml")
        && match zip.read("mimetype") {
            Ok(Some(m)) => m.starts_with(b"application/vnd.oasis.opendocument.spreadsheet"),
            _ => true,
        }
}

pub(crate) fn read(zip: &Archive<'_>) -> Result<Workbook, SheetError> {
    let text = zip.read_text("content.xml")?.ok_or(SheetError::Damaged)?;
    Ok(parse_content(&text))
}

#[derive(Default)]
struct PendingCell {
    kind: String,
    value: Option<String>,
    date: Option<String>,
    time: Option<String>,
    boolean: Option<String>,
    string: Option<String>,
    error: bool,
    /// Under a merged range: hidden, so not a value the person sees.
    covered: bool,
    repeat: usize,
    text: String,
    paragraphs: usize,
}

fn repeat_of(attrs: &str, key: &str) -> usize {
    attr(attrs, key)
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(1)
        .max(1)
}

fn parse_content(text: &str) -> Workbook {
    let mut book = Workbook::new();
    let mut sheet: Option<Sheet> = None;
    let mut row_at: usize = 0;
    let mut row_repeat: usize = 1;
    let mut row_cells: Vec<(usize, Cell)> = Vec::new();
    let mut col_at: usize = 0;
    let mut cell: Option<PendingCell> = None;
    let mut in_p = 0u32;
    let mut skip = 0u32; // inside an annotation (a comment on the cell)
    let mut depth_table = 0u32;

    for ev in xml::Reader::new(text) {
        match ev {
            Event::Start { name, attrs, empty } => {
                let tag = local(name);
                if skip > 0 {
                    if !empty {
                        skip += 1;
                    }
                    continue;
                }
                match tag {
                    "table" => {
                        depth_table += 1;
                        if depth_table == 1 {
                            sheet = Some(Sheet::new(&attr_text(attrs, "name").unwrap_or_default()));
                            row_at = 0;
                            if empty {
                                if let Some(s) = sheet.take() {
                                    book.sheets.push(s);
                                }
                                depth_table = 0;
                            }
                        }
                    }
                    "table-row" if depth_table == 1 => {
                        row_repeat = repeat_of(attrs, "number-rows-repeated");
                        row_cells.clear();
                        col_at = 0;
                        if empty {
                            row_at = row_at.saturating_add(row_repeat);
                        }
                    }
                    "table-cell" | "covered-table-cell" if depth_table == 1 => {
                        let c = PendingCell {
                            kind: attr_text(attrs, "value-type").unwrap_or_default(),
                            value: attr_text(attrs, "value"),
                            date: attr_text(attrs, "date-value"),
                            time: attr_text(attrs, "time-value"),
                            boolean: attr_text(attrs, "boolean-value"),
                            string: attr_text(attrs, "string-value"),
                            repeat: repeat_of(attrs, "number-columns-repeated"),
                            // LibreOffice marks a formula's error result so.
                            error: attrs.contains("calcext:value-type=\"error\""),
                            covered: tag == "covered-table-cell",
                            ..PendingCell::default()
                        };
                        if empty {
                            finish_cell(c, &mut col_at, &mut row_cells);
                        } else {
                            cell = Some(c);
                        }
                    }
                    "annotation" if !empty => skip = 1,
                    "p" | "h" if cell.is_some() => {
                        if let Some(c) = cell.as_mut() {
                            if c.paragraphs > 0 {
                                c.text.push('\n');
                            }
                            c.paragraphs += 1;
                        }
                        if !empty {
                            in_p += 1;
                        }
                    }
                    "s" if in_p > 0 => {
                        let n = repeat_of(attrs, "c").min(MAX_COLS);
                        if let Some(c) = cell.as_mut() {
                            for _ in 0..n {
                                c.text.push(' ');
                            }
                        }
                    }
                    "tab" if in_p > 0 => {
                        if let Some(c) = cell.as_mut() {
                            c.text.push('\t');
                        }
                    }
                    "line-break" if in_p > 0 => {
                        if let Some(c) = cell.as_mut() {
                            c.text.push('\n');
                        }
                    }
                    _ => {}
                }
            }
            Event::End { name } => {
                if skip > 0 {
                    skip -= 1;
                    continue;
                }
                match local(name) {
                    "table" if depth_table > 0 => {
                        depth_table -= 1;
                        if depth_table == 0 {
                            if let Some(s) = sheet.take() {
                                book.sheets.push(s);
                            }
                        }
                    }
                    "table-cell" | "covered-table-cell" if depth_table == 1 => {
                        if let Some(c) = cell.take() {
                            finish_cell(c, &mut col_at, &mut row_cells);
                        }
                    }
                    "table-row" if depth_table == 1 => {
                        if let Some(s) = sheet.as_mut() {
                            let copies = if row_cells.is_empty() {
                                0
                            } else {
                                row_repeat.min(MAX_FILLED_REPEAT)
                            };
                            for k in 0..copies {
                                let r = row_at + k;
                                if r >= MAX_ROWS {
                                    break;
                                }
                                for (col, value) in &row_cells {
                                    s.set(r, *col, value.clone());
                                }
                            }
                        }
                        row_at = row_at.saturating_add(row_repeat);
                        row_cells.clear();
                    }
                    "p" | "h" => in_p = in_p.saturating_sub(1),
                    _ => {}
                }
            }
            Event::Text(t) if in_p > 0 && skip == 0 => {
                if let Some(c) = cell.as_mut() {
                    push_unescaped(&mut c.text, t);
                }
            }
            Event::CData(t) if in_p > 0 && skip == 0 => {
                if let Some(c) = cell.as_mut() {
                    c.text.push_str(t);
                }
            }
            _ => {}
        }
    }
    book
}

fn finish_cell(c: PendingCell, col_at: &mut usize, row_cells: &mut Vec<(usize, Cell)>) {
    let repeat = c.repeat;
    let value = cell_value(c);
    if !matches!(value, Cell::Empty) {
        for k in 0..repeat.min(MAX_FILLED_REPEAT) {
            let col = *col_at + k;
            if col < MAX_COLS {
                row_cells.push((col, value.clone()));
            }
        }
    }
    *col_at = col_at.saturating_add(repeat);
}

fn cell_value(c: PendingCell) -> Cell {
    if c.covered {
        return Cell::Empty;
    }
    if c.error {
        return Cell::Error(c.text);
    }
    match c.kind.as_str() {
        "float" | "percentage" | "currency" => c
            .value
            .as_deref()
            .and_then(|v| v.trim().parse::<f64>().ok())
            .map(Cell::Number)
            .unwrap_or(Cell::Text(c.text)),
        "date" => c
            .date
            .as_deref()
            .and_then(DateTime::parse_iso)
            .map(Cell::Date)
            .unwrap_or(Cell::Text(c.text)),
        "time" => c
            .time
            .as_deref()
            .and_then(parse_duration)
            .map(Cell::Date)
            .unwrap_or(Cell::Text(c.text)),
        "boolean" => Cell::Bool(matches!(
            c.boolean.as_deref().map(str::trim),
            Some("true" | "1")
        )),
        _ => {
            let text = c.string.unwrap_or(c.text);
            if text.is_empty() {
                Cell::Empty
            } else {
                Cell::Text(text)
            }
        }
    }
}

/// `PT13H45M30S` (an ISO 8601 duration, how ODS stores a time) as a
/// time of day on 1899-12-31, the same shape an Excel time takes.
fn parse_duration(text: &str) -> Option<DateTime> {
    let body = text.trim().strip_prefix("PT")?;
    let mut total: f64 = 0.0;
    let mut number = String::new();
    for ch in body.chars() {
        match ch {
            'H' | 'M' | 'S' => {
                let n: f64 = number.parse().ok()?;
                total += n * match ch {
                    'H' => 3600.0,
                    'M' => 60.0,
                    _ => 1.0,
                };
                number.clear();
            }
            c => number.push(c),
        }
    }
    let secs = libm::round(total) as i64;
    let secs = secs.rem_euclid(86_400);
    Some(DateTime::date(1899, 12, 31).at(
        (secs / 3600) as u8,
        (secs / 60 % 60) as u8,
        (secs % 60) as u8,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_become_times() {
        assert_eq!(
            parse_duration("PT13H45M30S"),
            Some(DateTime::date(1899, 12, 31).at(13, 45, 30))
        );
        assert_eq!(
            parse_duration("PT0H0M0.6S"),
            Some(DateTime::date(1899, 12, 31).at(0, 0, 1))
        );
    }

    #[test]
    fn padding_repeats_do_not_allocate_a_million_rows() {
        let doc = r#"<office:document-content><office:body><office:spreadsheet>
            <table:table table:name="S"><table:table-row><table:table-cell office:value-type="float" office:value="1"><text:p>1</text:p></table:table-cell><table:table-cell table:number-columns-repeated="16383"/></table:table-row>
            <table:table-row table:number-rows-repeated="1048575"><table:table-cell table:number-columns-repeated="16384"/></table:table-row></table:table>
            </office:spreadsheet></office:body></office:document-content>"#;
        let book = parse_content(doc);
        assert_eq!(book.sheets.len(), 1);
        assert_eq!(book.sheets[0].height(), 1);
        assert_eq!(book.sheets[0].width(), 1);
    }
}
