//! Real files in, real files out (IC-993).
//!
//! The fixtures under `tests/fixtures/sheets/` are made by `make.py` there:
//! one written by openpyxl, one OpenDocument file in LibreOffice's shape.
//! The Excel-shaped workbook below is built part by part with the quirks
//! Excel itself writes (a shared string table with rich runs and phonetic
//! guides, formulas with cached values, errors, styles whose first list is
//! not the cell list, a prefixed namespace, cells without references, the
//! 1904 date system). What `write_xlsx` produces is read back here and by
//! calamine, an independent reader.

extern crate std;

use super::zip::Writer;
use super::*;
use alloc::{format, vec, vec::Vec};

const OPENPYXL: &[u8] = include_bytes!("../../tests/fixtures/sheets/openpyxl.xlsx");
const ODS: &[u8] = include_bytes!("../../tests/fixtures/sheets/office.ods");

fn t(s: &str) -> Cell {
    Cell::Text(String::from(s))
}

/// The rows both fixtures carry, as each reader should return them.
fn check_orders(sheet: &Sheet) {
    assert_eq!(sheet.name, "Orders");
    let header: Vec<String> = sheet.rows[0].iter().map(Cell::to_text).collect();
    assert_eq!(header, ["Item", "Qty", "Price", "Ordered", "Paid"]);
    assert_eq!(sheet.get(1, 0), &t("Paper A4"));
    assert_eq!(sheet.get(1, 1), &Cell::Number(10.0));
    assert_eq!(sheet.get(1, 2), &Cell::Number(4.95));
    assert_eq!(sheet.get(1, 3), &Cell::Date(DateTime::date(2024, 3, 15)));
    assert_eq!(sheet.get(1, 4), &Cell::Bool(true));
    assert_eq!(sheet.get(2, 0), &t("Zürich café ☕ 東京"));
    assert_eq!(sheet.get(2, 1), &Cell::Number(-3.0));
    assert_eq!(sheet.get(2, 2), &Cell::Number(0.1));
    assert_eq!(
        sheet.get(2, 3),
        &Cell::Date(DateTime::date(2024, 12, 31).at(23, 59, 58))
    );
    assert_eq!(sheet.get(2, 4), &Cell::Bool(false));
    assert_eq!(sheet.get(3, 0), &t(" spaced "));
    assert_eq!(sheet.get(3, 1), &Cell::Number(1e21));
    assert_eq!(sheet.get(3, 2), &Cell::Number(1.5e-7));
    assert_eq!(
        sheet.get(3, 3),
        &Cell::Date(DateTime::date(1899, 12, 31).at(9, 30, 0))
    );
    assert_eq!(sheet.get(3, 3).to_text(), "09:30:00");
    assert_eq!(sheet.get(3, 4), &Cell::Empty);
    assert!(sheet.rows[4].is_empty(), "row 5 is blank");
    assert_eq!(sheet.get(5, 0), &t("after a gap"));
    assert_eq!(sheet.get(5, 1), &Cell::Empty);
    assert_eq!(sheet.get(5, 4), &t("far right"));
    assert_eq!(sheet.height(), 6, "padding rows are not rows");
}

fn check_other_sheets(book: &Workbook) {
    let names: Vec<&str> = book.sheets.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["Orders", "Ünïcödé & <more>", "Empty"]);
    let second = &book.sheets[1];
    assert_eq!(second.get(1, 1), &t("only B2"));
    assert_eq!(second.get(0, 0), &Cell::Empty);
    assert_eq!(second.get(1, 2), &Cell::Empty, "nothing under a merge");
    assert_eq!(book.sheets[2].height(), 0);
    assert!(book.sheet("ünïcödé & <MORE>").is_some());
}

#[test]
fn reads_an_openpyxl_workbook() {
    let book = read(OPENPYXL).expect("openpyxl file reads");
    check_orders(&book.sheets[0]);
    // openpyxl saves a formula without a calculated value.
    assert_eq!(book.sheets[0].get(1, 5), &Cell::Empty);
    check_other_sheets(&book);
}

#[test]
fn reads_an_opendocument_workbook() {
    let book = read(ODS).expect("ods file reads");
    check_orders(&book.sheets[0]);
    // LibreOffice saves the formula's last value, and its error.
    assert_eq!(book.sheets[0].get(1, 5), &Cell::Number(49.5));
    assert_eq!(
        book.sheets[0].get(2, 5),
        &Cell::Error(String::from("#DIV/0!"))
    );
    assert_eq!(
        book.sheets[0].get(1, 0),
        &t("Paper A4"),
        "the comment is not the value"
    );
    check_other_sheets(&book);
}

const MAIN: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

/// A workbook in the shape Excel writes, part by part.
fn excel_shaped(date1904: bool) -> Vec<u8> {
    let mut z = Writer::new();
    z.add("[Content_Types].xml", b"<Types/>");
    z.add(
        "_rels/.rels",
        format!(r#"<Relationships><Relationship Id="rId1" Type="{REL}/officeDocument" Target="/xl/workbook.xml"/></Relationships>"#).as_bytes(),
    );
    z.add(
        "xl/workbook.xml",
        format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<x:workbook xmlns:x="{MAIN}" xmlns:r="{REL}"><x:workbookPr date1904="{}"/><x:sheets><x:sheet name="Data" sheetId="1" r:id="rId1"/><x:sheet name="Chart1" sheetId="2" r:id="rId9"/><x:sheet name="Q&amp;A" sheetId="3" r:id="rId2"/></x:sheets></x:workbook>"#,
            u8::from(date1904)
        )
        .as_bytes(),
    );
    z.add(
        "xl/_rels/workbook.xml.rels",
        format!(
            r#"<Relationships>
<Relationship Id="rId1" Type="{REL}/worksheet" Target="worksheets/sheet1.xml"/>
<Relationship Id="rId2" Type="{REL}/worksheet" Target="/xl/worksheets/sheet2.xml"/>
<Relationship Id="rId9" Type="{REL}/chartsheet" Target="chartsheets/sheet1.xml"/>
<Relationship Id="rId3" Type="{REL}/sharedStrings" Target="sharedStrings.xml"/>
<Relationship Id="rId4" Type="{REL}/styles" Target="styles.xml"/></Relationships>"#
        )
        .as_bytes(),
    );
    z.add(
        "xl/sharedStrings.xml",
        format!(r#"<sst xmlns="{MAIN}" count="4" uniqueCount="4"><si><t>Name</t></si><si><r><rPr><b/></rPr><t>Bold</t></r><r><t xml:space="preserve"> and plain</t></r></si><si><t>東京</t><rPh sb="0" eb="2"><t>トウキョウ</t></rPh></si><si><t>line_x000D_
two &amp; &lt;three&gt;</t></si></sst>"#).as_bytes(),
    );
    // cellStyleXfs comes first and must not be mistaken for the cell list:
    // its xf has a date format, cellXfs index 0 does not.
    z.add(
        "xl/styles.xml",
        format!(r#"<styleSheet xmlns="{MAIN}"><numFmts count="1"><numFmt numFmtId="170" formatCode="[$-407]dd/mm/yyyy;@"/></numFmts>
<cellStyleXfs count="1"><xf numFmtId="14"/></cellStyleXfs>
<cellXfs count="4"><xf numFmtId="0" xfId="0"/><xf numFmtId="170" xfId="0" applyNumberFormat="1"><alignment horizontal="left"/></xf><xf numFmtId="4" xfId="0"/><xf numFmtId="22" xfId="0"/></cellXfs></styleSheet>"#).as_bytes(),
    );
    z.add(
        "xl/worksheets/sheet1.xml",
        format!(r#"<worksheet xmlns="{MAIN}"><dimension ref="A1:D4"/><sheetData>
<row r="1" spans="1:4"><c r="A1" t="s"><v>0</v></c><c r="B1" t="s"><v>1</v></c><c r="C1" t="s"><v>2</v></c><c r="D1" t="s"><v>3</v></c></row>
<row r="2"><c r="A2" s="1"><v>45366</v></c><c r="B2" s="2"><v>1234.5</v></c><c r="C2"><f>B2*2</f><v>2469</v></c><c r="D2" t="str"><f>"x"&amp;"y"</f><v>xy</v></c></row>
<row r="3"><c r="A3" t="e"><f>1/0</f><v>#DIV/0!</v></c><c r="B3" t="b"><v>1</v></c><c r="C3" s="3"><v>45366.75</v></c><c r="D3" s="1"/></row>
<row><c><v>7</v></c><c><v>8</v></c><c r="F4" t="inlineStr"><is><t>inline</t></is></c></row>
</sheetData></worksheet>"#).as_bytes(),
    );
    z.add(
        "xl/worksheets/sheet2.xml",
        format!(r#"<worksheet xmlns="{MAIN}"><sheetData><row r="3"><c r="C3" t="s"><v>0</v></c><c r="D3" t="s"><v>1</v></c></row></sheetData><mergeCells count="1"><mergeCell ref="C3:D3"/></mergeCells></worksheet>"#).as_bytes(),
    );
    z.finish()
}

#[test]
fn reads_what_excel_writes() {
    let book = read(&excel_shaped(false)).expect("excel-shaped file reads");
    let names: Vec<&str> = book.sheets.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["Data", "Q&A"], "the chart sheet has no cells");
    let s = &book.sheets[0];
    assert_eq!(s.get(0, 0), &t("Name"));
    assert_eq!(s.get(0, 1), &t("Bold and plain"));
    assert_eq!(s.get(0, 2), &t("東京"), "the reading guide is not text");
    assert_eq!(s.get(0, 3), &t("line\r\ntwo & <three>"));
    assert_eq!(s.get(1, 0), &Cell::Date(DateTime::date(2024, 3, 15)));
    assert_eq!(s.get(1, 1), &Cell::Number(1234.5));
    assert_eq!(
        s.get(1, 2),
        &Cell::Number(2469.0),
        "a formula's cached value"
    );
    assert_eq!(s.get(1, 3), &t("xy"));
    assert_eq!(s.get(2, 0), &Cell::Error(String::from("#DIV/0!")));
    assert_eq!(s.get(2, 1), &Cell::Bool(true));
    assert_eq!(
        s.get(2, 2),
        &Cell::Date(DateTime::date(2024, 3, 15).at(18, 0, 0))
    );
    assert_eq!(s.get(2, 3), &Cell::Empty, "a styled cell with no value");
    assert_eq!(s.get(3, 0), &Cell::Number(7.0), "cells without references");
    assert_eq!(s.get(3, 1), &Cell::Number(8.0));
    assert_eq!(s.get(3, 5), &t("inline"));
    assert_eq!(book.sheets[1].get(2, 2), &t("Name"));
    assert_eq!(
        book.sheets[1].get(2, 3),
        &Cell::Empty,
        "a value hidden under a merged range is not shown"
    );
}

#[test]
fn the_1904_date_system_is_honoured() {
    let book = read(&excel_shaped(true)).expect("reads");
    // 45366 days after 1904-01-01.
    assert_eq!(
        book.sheets[0].get(1, 0),
        &Cell::Date(DateTime::date(2028, 3, 16))
    );
}

fn everything() -> Workbook {
    let mut a = Sheet::new("Orders");
    a.push_row(vec![t("Item"), t("Qty"), t("When"), t("Paid"), t("Note")]);
    a.push_row(vec![
        t("Zürich café ☕ 東京 🚀"),
        Cell::from(10),
        Cell::Date(DateTime::date(2024, 2, 29)),
        Cell::from(true),
        t("a & b < c > d \"q\""),
    ]);
    a.push_row(vec![
        t(" spaced "),
        Cell::from(0.1 + 0.2),
        Cell::Date(DateTime::date(1999, 12, 31).at(23, 59, 59)),
        Cell::from(false),
        t("line\nbreak\ttab"),
    ]);
    a.push_row(vec![
        t("Item"),
        Cell::from(-1.5e-7),
        Cell::Date(DateTime::date(1899, 12, 31).at(7, 5, 0)),
        Cell::Error(String::from("#N/A")),
        Cell::from(1e21),
    ]);
    a.push_row(Vec::new());
    a.set(5, 30, t("column AE"));
    let mut b = Sheet::new("Sheet/With:Bad*Name?that is far too long for excel");
    b.set(0, 0, Cell::from(1900.0));
    let c = Sheet::new("orders"); // same name as the first, ignoring case
    Workbook {
        sheets: vec![a, b, c],
    }
}

#[test]
fn what_it_writes_reads_back_the_same() {
    let book = everything();
    let bytes = write_xlsx(&book);
    let back = read(&bytes).expect("own output reads");
    assert_eq!(back.sheets[0].rows, book.sheets[0].rows);
    assert_eq!(back.sheets[1].get(0, 0), &Cell::Number(1900.0));
    let names: Vec<&str> = back.sheets.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        names,
        ["Orders", "Sheet_With_Bad_Name_that is far", "orders (2)"]
    );
    assert!(names.iter().all(|n| n.chars().count() <= 31));
    assert_eq!(write_xlsx(&book), bytes, "same workbook, same bytes");
}

#[test]
fn an_empty_workbook_still_opens() {
    let back = read(&write_xlsx(&Workbook::new())).expect("reads");
    assert_eq!(back.sheets.len(), 1);
    assert_eq!(back.sheets[0].name, "Sheet1");
}

#[test]
fn fixtures_survive_a_round_trip() {
    for fixture in [OPENPYXL, ODS] {
        let book = read(fixture).expect("reads");
        let again = read(&write_xlsx(&book)).expect("rewritten file reads");
        assert_eq!(again, book);
    }
}

/// calamine (MIT, the reader `krate port` uses) reads what we write, so the
/// file is not only readable by our own reader.
#[test]
fn an_independent_reader_agrees() {
    use calamine::{Data, Reader, Xlsx};
    let bytes = write_xlsx(&everything());
    let mut xl: Xlsx<_> = Xlsx::new(std::io::Cursor::new(bytes)).expect("calamine opens it");
    assert_eq!(
        xl.sheet_names(),
        ["Orders", "Sheet_With_Bad_Name_that is far", "orders (2)"]
    );
    let range = xl.worksheet_range("Orders").expect("sheet reads");
    let cell = |r: u32, c: u32| range.get_value((r, c)).cloned().unwrap_or(Data::Empty);
    assert_eq!(
        cell(1, 0),
        Data::String(String::from("Zürich café ☕ 東京 🚀"))
    );
    assert_eq!(cell(1, 1), Data::Float(10.0));
    match cell(1, 2) {
        Data::DateTime(d) => {
            assert!(d.is_datetime());
            assert_eq!(d.as_f64(), DateTime::date(2024, 2, 29).to_serial());
        }
        other => panic!("a date, not {other:?}"),
    }
    assert_eq!(cell(1, 3), Data::Bool(true));
    assert_eq!(
        cell(1, 4),
        Data::String(String::from("a & b < c > d \"q\""))
    );
    assert_eq!(cell(2, 0), Data::String(String::from(" spaced ")));
    assert_eq!(cell(2, 1), Data::Float(0.1 + 0.2));
    assert_eq!(cell(2, 4), Data::String(String::from("line\nbreak\ttab")));
    assert_eq!(cell(3, 1), Data::Float(-1.5e-7));
    assert!(matches!(cell(3, 3), Data::Error(_)));
    assert_eq!(cell(3, 4), Data::Float(1e21));
    assert_eq!(cell(5, 30), Data::String(String::from("column AE")));
}

#[test]
fn csv_round_trips_typed_cells_as_text() {
    let book = everything();
    let csv = write_csv(&book.sheets[0]);
    assert!(csv.starts_with('\u{feff}'));
    assert!(csv.contains("2024-02-29,TRUE"));
    let back = read(csv.as_bytes()).expect("csv reads");
    let s = &back.sheets[0];
    assert_eq!(s.get(1, 0), &t("Zürich café ☕ 東京 🚀"));
    assert_eq!(s.get(1, 1).as_number(), Some(10.0));
    assert_eq!(s.get(2, 1).as_number(), Some(0.1 + 0.2), "exact, not 0.3");
    assert_eq!(s.get(2, 4), &t("line\nbreak\ttab"));
    assert_eq!(s.get(3, 4).as_number(), Some(1e21));
    assert_eq!(s.get(5, 30), &t("column AE"));
}

#[test]
fn wrong_files_get_plain_answers() {
    assert_eq!(
        read(&[0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1, 0, 0]),
        Err(SheetError::OldExcel)
    );
    assert_eq!(
        read(b"\x89PNG\r\n\x1a\n\0\0"),
        Err(SheetError::NotASpreadsheet)
    );
    let mut docx = Writer::new();
    docx.add("word/document.xml", b"<w/>");
    assert_eq!(read(&docx.finish()), Err(SheetError::NotASpreadsheet));
    let good = write_xlsx(&everything());
    assert_eq!(read(&good[..good.len() / 2]), Err(SheetError::Damaged));
    assert!(SheetError::OldExcel.message().contains(".xlsx"));
}

#[test]
fn a_damaged_workbook_never_panics() {
    let good = write_xlsx(&everything());
    for i in (0..good.len()).step_by(7) {
        let mut bad = good.clone();
        bad[i] ^= 0xa5;
        let _ = read(&bad);
    }
}

#[test]
fn dates_and_serials_agree_with_excel() {
    // Excel's own serials, including both sides of its 1900 leap-year bug.
    for (serial, y, m, d) in [
        (1.0, 1900, 1, 1),
        (59.0, 1900, 2, 28),
        (61.0, 1900, 3, 1),
        (25569.0, 1970, 1, 1),
        (36526.0, 2000, 1, 1),
        (45366.0, 2024, 3, 15),
        (2958465.0, 9999, 12, 31),
    ] {
        let date = DateTime::date(y, m, d);
        assert_eq!(DateTime::from_serial(serial), date, "{serial}");
        assert_eq!(date.to_serial(), serial, "{y}-{m}-{d}");
    }
    let noon = DateTime::from_serial(45366.5);
    assert_eq!(noon, DateTime::date(2024, 3, 15).at(12, 0, 0));
    assert_eq!(noon.to_text(), "2024-03-15 12:00:00");
    assert_eq!(noon.to_iso(), "2024-03-15T12:00:00");
    assert_eq!(DateTime::parse_iso("2024-03-15T12:00:00.123Z"), Some(noon));
    assert_eq!(
        DateTime::date(2024, 3, 15).day_number(),
        crate::locale::day_of_date(2024, 3, 15)
    );
}

#[test]
fn numbers_show_the_way_a_spreadsheet_shows_them() {
    assert_eq!(Cell::from(3.0).to_text(), "3");
    assert_eq!(Cell::from(0.1 + 0.2).to_text(), "0.3");
    assert_eq!(Cell::from(-0.0).to_text(), "0");
    assert_eq!(Cell::from(1e21).to_text(), "1E21");
    assert_eq!(Cell::from(1234567.25).to_text(), "1234567.25");
    assert_eq!(number_exact(0.1 + 0.2), "0.30000000000000004");
    assert_eq!(number_exact(1e21), "1E+21");
    assert_eq!(number_exact(-1.5e-7), "-1.5E-7");
    assert_eq!(Sheet::column_name(0), "A");
    assert_eq!(Sheet::column_name(27), "AB");
    assert_eq!(Sheet::column_name(16_383), "XFD");
    assert_eq!(column_index("XFD"), Some(16_383));
}
