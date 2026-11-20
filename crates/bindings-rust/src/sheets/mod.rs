//! Spreadsheets: read the workbook a person opens, write the one they save.
//!
//! Turn on the `sheets` feature of the `krate` dependency to use this
//! module. It runs inside the app -- pure Rust, no host call, no capability
//! of its own -- so it works the same on every computer, in a browser tab
//! and on a phone. The bytes come from wherever the person handed them
//! over: `krate::fs::read_chosen` after `ui.dialog:file-open` or a drop,
//! and go back with `krate::fs::write_chosen` after `ui.dialog:file-save`.
//!
//! ```no_run
//! # fn demo(token: &str, save_token: &str) -> Result<(), krate::sheets::SheetError> {
//! use krate::sheets::{self, Cell};
//! let bytes = krate::fs::read_chosen(token).unwrap_or_default();
//! let book = sheets::read(&bytes)?; // .xlsx, .ods or .csv
//! if let Some(sheet) = book.sheets.first() {
//!     for row in &sheet.rows {
//!         for cell in row {
//!             let _shown = cell.to_text(); // what the cell shows
//!         }
//!     }
//! }
//! let mut out = sheets::Sheet::new("Totals");
//! out.push_row(vec![Cell::from("Item"), Cell::from("Amount")]);
//! out.push_row(vec![Cell::from("Paper"), Cell::from(12.5)]);
//! let xlsx = sheets::write_xlsx(&sheets::Workbook { sheets: vec![out] });
//! let _ = krate::fs::write_chosen(save_token, &xlsx);
//! # Ok(()) }
//! ```
//!
//! What comes through: every sheet in order with its name; text (shared,
//! inline and rich), numbers, true/false, dates and times (recognised from
//! the cell's number format, in the 1900 and 1904 systems), the error a
//! cell shows (`#DIV/0!`), and for a formula the value it last calculated.
//! Formulas themselves, formatting, merged cells and charts are not read.
//! An old binary `.xls` is recognised and refused with words that say to
//! save it as `.xlsx`.

use alloc::{
    string::{String, ToString},
    vec::Vec,
};
use core::fmt::Write as _;

mod csv;
mod ods;
mod xlsx;
mod xml;
mod zip;

pub use csv::{read_csv, write_csv};
pub use xlsx::write_xlsx;

/// One cell's value.
#[derive(Clone, Debug, PartialEq)]
pub enum Cell {
    /// Nothing in the cell.
    Empty,
    /// Text, exactly as typed.
    Text(String),
    /// A number. Whole numbers are numbers too: `3.0` shows as `3`.
    Number(f64),
    /// TRUE or FALSE.
    Bool(bool),
    /// A date, a time, or both.
    Date(DateTime),
    /// The error the cell shows, such as `#DIV/0!` or `#N/A`.
    Error(String),
}

/// A calendar date and a time of day, as the sheet shows it (no time zone).
///
/// A time with no date (a cell formatted `h:mm`) comes through as
/// 1899-12-31, the day before Excel's day 1, and [`DateTime::to_text`]
/// shows only the time for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DateTime {
    pub year: i32,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

/// One sheet: a name and its rows, top to bottom.
///
/// `rows[r][c]` is the cell in row `r + 1`, column `c + 1` (A is 0). Rows
/// can differ in length -- a row ends at its last filled cell -- so read
/// with [`Sheet::get`], which answers [`Cell::Empty`] past the end.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sheet {
    pub name: String,
    pub rows: Vec<Vec<Cell>>,
}

/// Every sheet in a workbook, in tab order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Workbook {
    pub sheets: Vec<Sheet>,
}

/// Why a file could not be read as a spreadsheet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SheetError {
    /// Not a workbook or a CSV at all (a picture, a PDF, a Word file...).
    NotASpreadsheet,
    /// An Excel 97-2003 `.xls` file.
    OldExcel,
    /// A workbook whose insides are broken or cut short.
    Damaged,
    /// A part that would unpack to more than half a gigabyte.
    TooLarge,
}

impl SheetError {
    /// Plain words to show the person.
    pub fn message(self) -> &'static str {
        match self {
            SheetError::NotASpreadsheet => {
                "This file is not a spreadsheet. Open an .xlsx, .ods or .csv file."
            }
            SheetError::OldExcel => {
                "This is an old Excel (.xls) file. Open it in Excel or LibreOffice, save it as .xlsx, and open that."
            }
            SheetError::Damaged => {
                "This spreadsheet is damaged or incomplete. Try saving it again from the program that made it."
            }
            SheetError::TooLarge => "This spreadsheet is too large to open here.",
        }
    }
}

impl core::fmt::Display for SheetError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.message())
    }
}

/// Read a workbook from the bytes of an `.xlsx`, `.xlsm`, `.ods` or `.csv`
/// file. The format is recognised from the bytes, never from the name.
///
/// A CSV becomes one sheet called "Sheet1" whose cells are all
/// [`Cell::Text`] (CSV carries no types; [`Cell::as_number`] reads a
/// number out of text). CSV text that is not UTF-8 is read as Windows-1252,
/// which is what Excel on Windows saves.
pub fn read(bytes: &[u8]) -> Result<Workbook, SheetError> {
    if bytes.starts_with(&[0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1]) {
        return Err(SheetError::OldExcel);
    }
    if zip::is_zip(bytes) {
        let archive = zip::Archive::open(bytes)?;
        if xlsx::looks_like(&archive) {
            return xlsx::read(&archive);
        }
        if ods::looks_like(&archive) {
            return ods::read(&archive);
        }
        return Err(SheetError::NotASpreadsheet);
    }
    if bytes.contains(&0) {
        return Err(SheetError::NotASpreadsheet);
    }
    let text = match core::str::from_utf8(bytes) {
        Ok(text) => String::from(text),
        Err(_) => csv::decode_1252(bytes),
    };
    let mut sheet = read_csv(&text);
    sheet.name = String::from("Sheet1");
    Ok(Workbook {
        sheets: alloc::vec![sheet],
    })
}

impl Workbook {
    /// An empty workbook.
    pub fn new() -> Self {
        Workbook { sheets: Vec::new() }
    }

    /// The sheet called `name` (ignoring case), if there is one.
    pub fn sheet(&self, name: &str) -> Option<&Sheet> {
        self.sheets
            .iter()
            .find(|s| s.name.to_lowercase() == name.to_lowercase())
    }
}

static EMPTY: Cell = Cell::Empty;

impl Sheet {
    /// An empty sheet with this name.
    pub fn new(name: &str) -> Self {
        Sheet {
            name: String::from(name),
            rows: Vec::new(),
        }
    }

    /// The cell at `row`, `col` (both from 0), or [`Cell::Empty`].
    pub fn get(&self, row: usize, col: usize) -> &Cell {
        self.rows
            .get(row)
            .and_then(|r| r.get(col))
            .unwrap_or(&EMPTY)
    }

    /// Put `cell` at `row`, `col` (both from 0), growing the sheet to fit.
    pub fn set(&mut self, row: usize, col: usize, cell: Cell) {
        if self.rows.len() <= row {
            self.rows.resize(row + 1, Vec::new());
        }
        if let Some(r) = self.rows.get_mut(row) {
            if r.len() <= col {
                r.resize(col + 1, Cell::Empty);
            }
            if let Some(slot) = r.get_mut(col) {
                *slot = cell;
            }
        }
    }

    /// Add a row at the bottom.
    pub fn push_row(&mut self, row: Vec<Cell>) {
        self.rows.push(row);
    }

    /// How many rows there are.
    pub fn height(&self) -> usize {
        self.rows.len()
    }

    /// How many columns the widest row has.
    pub fn width(&self) -> usize {
        self.rows.iter().map(Vec::len).max().unwrap_or(0)
    }

    /// The column letters for a column index: 0 -> "A", 27 -> "AB".
    pub fn column_name(col: usize) -> String {
        let mut n = col + 1;
        let mut out = Vec::new();
        while n > 0 {
            let rem = (n - 1) % 26;
            out.push(b'A' + rem as u8);
            n = (n - 1) / 26;
        }
        out.reverse();
        String::from_utf8(out).unwrap_or_default()
    }
}

impl Cell {
    /// True for [`Cell::Empty`], and for text that is only spaces.
    pub fn is_empty(&self) -> bool {
        match self {
            Cell::Empty => true,
            Cell::Text(t) => t.trim().is_empty(),
            _ => false,
        }
    }

    /// The number in the cell: a number, or text that reads as one
    /// (`" 12.50 "`, `"1e3"`). `None` for anything else.
    pub fn as_number(&self) -> Option<f64> {
        match self {
            Cell::Number(n) => Some(*n),
            Cell::Text(t) => t.trim().parse::<f64>().ok().filter(|n| n.is_finite()),
            _ => None,
        }
    }

    /// The text in the cell, if it holds text.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Cell::Text(t) => Some(t),
            _ => None,
        }
    }

    /// What the cell shows, as plain text: numbers to 15 significant
    /// digits the way a spreadsheet shows them (`0.1 + 0.2` shows `0.3`),
    /// TRUE/FALSE, dates as `2024-03-15` or `2024-03-15 09:30:00`.
    pub fn to_text(&self) -> String {
        match self {
            Cell::Empty => String::new(),
            Cell::Text(t) | Cell::Error(t) => t.clone(),
            Cell::Number(n) => number_text(*n),
            Cell::Bool(true) => String::from("TRUE"),
            Cell::Bool(false) => String::from("FALSE"),
            Cell::Date(d) => d.to_text(),
        }
    }
}

impl From<&str> for Cell {
    fn from(s: &str) -> Self {
        Cell::Text(String::from(s))
    }
}
impl From<String> for Cell {
    fn from(s: String) -> Self {
        Cell::Text(s)
    }
}
impl From<f64> for Cell {
    fn from(n: f64) -> Self {
        Cell::Number(n)
    }
}
impl From<f32> for Cell {
    fn from(n: f32) -> Self {
        Cell::Number(f64::from(n))
    }
}
impl From<i64> for Cell {
    fn from(n: i64) -> Self {
        Cell::Number(n as f64)
    }
}
impl From<i32> for Cell {
    fn from(n: i32) -> Self {
        Cell::Number(f64::from(n))
    }
}
impl From<u32> for Cell {
    fn from(n: u32) -> Self {
        Cell::Number(f64::from(n))
    }
}
impl From<usize> for Cell {
    fn from(n: usize) -> Self {
        Cell::Number(n as f64)
    }
}
impl From<bool> for Cell {
    fn from(b: bool) -> Self {
        Cell::Bool(b)
    }
}
impl From<DateTime> for Cell {
    fn from(d: DateTime) -> Self {
        Cell::Date(d)
    }
}

/// Day number (days since 1970-01-01) of 1899-12-30, Excel's day 0 for
/// every serial from 61 up.
const EXCEL_EPOCH: i64 = -25_569;

impl DateTime {
    /// A date at midnight.
    pub fn date(year: i32, month: u8, day: u8) -> Self {
        DateTime {
            year,
            month,
            day,
            hour: 0,
            minute: 0,
            second: 0,
        }
    }

    /// The same date with a time of day.
    pub fn at(self, hour: u8, minute: u8, second: u8) -> Self {
        DateTime {
            hour,
            minute,
            second,
            ..self
        }
    }

    /// Days since 1970-01-01, the same count as `krate::locale::today()`,
    /// so "is this due before today" is one comparison.
    pub fn day_number(&self) -> i64 {
        crate::locale::day_of_date(self.year, self.month, self.day)
    }

    /// The date for a day number (see [`DateTime::day_number`]).
    pub fn from_day_number(day: i64) -> Self {
        let (year, month, day) = crate::locale::date_of_day(day);
        DateTime::date(year, month, day)
    }

    /// True when the value carries a date, not only a time of day.
    pub fn has_date(&self) -> bool {
        !(self.year == 1899 && self.month == 12 && (self.day == 30 || self.day == 31))
    }

    /// From an Excel date serial (1900 date system): 45366.5 is
    /// 2024-03-15 12:00. Times round to the nearest second.
    pub fn from_serial(serial: f64) -> Self {
        Self::from_serial_in(serial, false)
    }

    pub(crate) fn from_serial_in(serial: f64, date1904: bool) -> Self {
        let mut seconds = libm::round(serial * 86_400.0) as i64;
        let mut days = seconds.div_euclid(86_400);
        seconds = seconds.rem_euclid(86_400);
        if date1904 {
            days += 1462; // 1904-01-01 is day 1462 of the 1900 system
        }
        // Excel counts a 29 February 1900 that never was: below serial 61
        // the count starts a day later.
        let base = if days < 60 {
            EXCEL_EPOCH + 1
        } else {
            EXCEL_EPOCH
        };
        let mut out = DateTime::from_day_number(base + days);
        out.hour = (seconds / 3600) as u8;
        out.minute = (seconds / 60 % 60) as u8;
        out.second = (seconds % 60) as u8;
        out
    }

    /// The Excel date serial (1900 date system) for this date and time.
    pub fn to_serial(&self) -> f64 {
        let offset = self.day_number() - EXCEL_EPOCH;
        let days = if !self.has_date() {
            0
        } else if offset < 61 {
            offset - 1
        } else {
            offset
        };
        let secs =
            i64::from(self.hour) * 3600 + i64::from(self.minute) * 60 + i64::from(self.second);
        days as f64 + secs as f64 / 86_400.0
    }

    /// `2024-03-15`, `2024-03-15 09:30:00`, or `09:30:00` for a time alone.
    pub fn to_text(&self) -> String {
        let mut s = String::new();
        let has_time = self.hour != 0 || self.minute != 0 || self.second != 0;
        if self.has_date() {
            let _ = write!(s, "{:04}-{:02}-{:02}", self.year, self.month, self.day);
            if has_time {
                s.push(' ');
            }
        }
        if has_time || !self.has_date() {
            let _ = write!(s, "{:02}:{:02}:{:02}", self.hour, self.minute, self.second);
        }
        s
    }

    /// `2024-03-15` or `2024-03-15T09:30:00`, the ISO 8601 form CSV and
    /// other programs read back as a date.
    pub fn to_iso(&self) -> String {
        let mut s = String::new();
        let _ = write!(s, "{:04}-{:02}-{:02}", self.year, self.month, self.day);
        if self.hour != 0 || self.minute != 0 || self.second != 0 {
            let _ = write!(s, "T{:02}:{:02}:{:02}", self.hour, self.minute, self.second);
        }
        s
    }

    /// Read `2024-03-15`, `2024-03-15T09:30:00` or `2024-03-15 09:30`
    /// (fractions of a second and a trailing zone are ignored).
    pub fn parse_iso(text: &str) -> Option<Self> {
        let text = text.trim();
        let (date, time) = match text.find(['T', ' ']) {
            Some(at) => (text.get(..at)?, text.get(at + 1..)),
            None => (text, None),
        };
        let mut parts = date.splitn(3, '-');
        let year: i32 = parts.next()?.parse().ok()?;
        let month: u8 = parts.next()?.parse().ok()?;
        let day: u8 = parts.next()?.parse().ok()?;
        if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
            return None;
        }
        let mut out = DateTime::date(year, month, day);
        if let Some(time) = time {
            let time = time.trim_end_matches('Z');
            let time = time.get(..time.find(['+', '.']).unwrap_or(time.len()))?;
            let mut t = time.splitn(3, ':');
            out.hour = t.next()?.parse().ok()?;
            out.minute = t.next().unwrap_or("0").parse().ok()?;
            out.second = t.next().unwrap_or("0").parse().ok()?;
        }
        Some(out)
    }
}

/// A number the way a spreadsheet shows it: 15 significant digits, no
/// trailing zeros, no exponent for everyday sizes.
fn number_text(n: f64) -> String {
    if !n.is_finite() {
        return String::from("#NUM!");
    }
    let mut rounded = String::new();
    let _ = write!(rounded, "{n:.14e}");
    let n = rounded.parse::<f64>().unwrap_or(n);
    let abs = n.abs();
    let mut s = String::new();
    if abs != 0.0 && !(1e-9..1e15).contains(&abs) {
        let _ = write!(s, "{n:e}");
        return s.replace('e', "E");
    }
    let _ = write!(s, "{n}");
    if s == "-0" {
        s = String::from("0");
    }
    s
}

/// A number written so it reads back to exactly the same `f64`.
pub(crate) fn number_exact(n: f64) -> String {
    let mut s = String::new();
    if !n.is_finite() {
        return String::from("0");
    }
    let abs = n.abs();
    if abs != 0.0 && !(1e-6..1e16).contains(&abs) {
        let _ = write!(s, "{n:e}");
        // Excel writes 1E+21; keep that shape.
        if let Some(at) = s.find('e') {
            let (mantissa, exp) = s.split_at(at);
            let exp = exp.get(1..).unwrap_or("0");
            let sign = if exp.starts_with('-') { "" } else { "+" };
            return [mantissa, "E", sign, exp].concat();
        }
        return s;
    }
    let _ = write!(s, "{n}");
    s
}

/// Excel's column letters to an index: "A" -> 0, "AB" -> 27.
pub(crate) fn column_index(letters: &str) -> Option<usize> {
    let mut n: usize = 0;
    for b in letters.bytes() {
        if !b.is_ascii_alphabetic() {
            return None;
        }
        n = n
            .checked_mul(26)?
            .checked_add(usize::from(b.to_ascii_uppercase() - b'A') + 1)?;
    }
    n.checked_sub(1)
}

/// Make `name` a sheet name Excel accepts: 1 to 31 characters, none of
/// `[]:*?/\`, not starting or ending with an apostrophe, and unique among
/// `taken` (compared ignoring case).
pub(crate) fn clean_sheet_name(name: &str, taken: &[String]) -> String {
    let mut base: String = name
        .chars()
        .map(|c| match c {
            '[' | ']' | ':' | '*' | '?' | '/' | '\\' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    base = base.trim_matches('\'').trim().to_string();
    if base.is_empty() || base.eq_ignore_ascii_case("history") {
        base = String::from("Sheet");
    }
    let base: String = base.chars().take(31).collect();
    let free = |c: &str| !taken.iter().any(|t| t.to_lowercase() == c.to_lowercase());
    if free(&base) {
        return base;
    }
    let mut n = 2u32;
    loop {
        let mut suffix = String::new();
        let _ = write!(suffix, " ({n})");
        let keep = 31usize.saturating_sub(suffix.chars().count());
        let candidate: String = base.chars().take(keep).chain(suffix.chars()).collect();
        if free(&candidate) {
            return candidate;
        }
        n += 1;
    }
}

#[cfg(test)]
mod tests;
