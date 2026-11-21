// The smallest app that reads and writes spreadsheets with krate::sheets
// (IC-993), so the whole path -- unzip, XML, typed cells, writing a new
// .xlsx and a CSV, reading both back -- is proved inside the sandbox, as
// WebAssembly, with nothing but krate:* imports.
//
//   krate check-app apps/krate-sheets-probe
//
// It reads two real files carried in the app (one written by openpyxl, one
// OpenDocument file in LibreOffice's shape), writes them out again, and
// prints one line per check, then `sheets=ok` or `sheets=FAILED`.
#![no_std]
extern crate alloc;

use alloc::{format, string::String, vec, vec::Vec};
use krate::{
    io::{stdio, streams::OutputStreamExt},
    sheets::{self, Cell, DateTime, Sheet, Workbook},
    Guest,
};

const XLSX: &[u8] =
    include_bytes!("../../../crates/bindings-rust/tests/fixtures/sheets/openpyxl.xlsx");
const ODS: &[u8] = include_bytes!("../../../crates/bindings-rust/tests/fixtures/sheets/office.ods");

struct Component;

fn describe(book: &Workbook) -> String {
    let names: Vec<&str> = book.sheets.iter().map(|s| s.name.as_str()).collect();
    let Some(first) = book.sheets.first() else {
        return String::from("no sheets");
    };
    format!(
        "sheets={} first={}x{} A3={} B2={} D2={} D3={}",
        names.join("|"),
        first.height(),
        first.width(),
        first.get(2, 0).to_text(),
        first.get(1, 1).to_text(),
        first.get(1, 3).to_text(),
        first.get(2, 3).to_text(),
    )
}

impl Guest for Component {
    fn run() -> i32 {
        let out = stdio::stdout();
        let say = |line: &str| {
            let _ = out.write_line(line);
            let _ = out.flush();
        };
        let mut ok = true;
        let rest = "A3=Zürich café ☕ 東京 B2=10 D2=2024-03-15 D3=2024-12-31 23:59:58";
        let names = "sheets=Orders|Ünïcödé & <more>|Empty";

        // The .ods also keeps a formula's last value in column F; openpyxl
        // saves none, so its first sheet is a column narrower.
        for (label, bytes, size) in [("xlsx", XLSX, "6x5"), ("ods", ODS, "6x6")] {
            let expect = format!("{names} first={size} {rest}");
            match sheets::read(bytes) {
                Ok(book) => {
                    let seen = describe(&book);
                    let good = seen == expect;
                    ok &= good;
                    say(&format!(
                        "{label} {} {seen}",
                        if good { "ok" } else { "WRONG" }
                    ));
                    // Out and back in: the rewritten file holds the same cells.
                    let again = sheets::read(&sheets::write_xlsx(&book));
                    let same = again.as_ref() == Ok(&book);
                    ok &= same;
                    say(&format!(
                        "{label} rewritten {}",
                        if same { "same" } else { "DIFFERENT" }
                    ));
                }
                Err(e) => {
                    ok = false;
                    say(&format!("{label} FAILED {}", e.message()));
                }
            }
        }

        // A workbook made in the app, saved as .xlsx and as .csv.
        let mut sheet = Sheet::new("Made here");
        sheet.push_row(vec![
            Cell::from("Item"),
            Cell::from("Amount"),
            Cell::from("Due"),
        ]);
        sheet.push_row(vec![
            Cell::from("Paper, A4"),
            Cell::from(12.5),
            Cell::Date(DateTime::date(2026, 11, 30)),
        ]);
        let xlsx = sheets::write_xlsx(&Workbook {
            sheets: vec![sheet.clone()],
        });
        let back = sheets::read(&xlsx).map(|b| b.sheets);
        let same = back.as_ref().map(|s| s.first() == Some(&sheet)) == Ok(true);
        ok &= same;
        say(&format!(
            "made xlsx={} bytes {}",
            xlsx.len(),
            if same { "same" } else { "DIFFERENT" }
        ));
        let csv = sheets::write_csv(&sheet);
        let read = sheets::read_csv(&csv);
        let csv_ok = read.get(1, 0).as_str() == Some("Paper, A4")
            && read.get(1, 1).as_number() == Some(12.5)
            && read.get(1, 2).as_str() == Some("2026-11-30");
        ok &= csv_ok;
        say(&format!(
            "made csv {}",
            if csv_ok { "same" } else { "DIFFERENT" }
        ));

        let old = sheets::read(&[0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1]);
        let refused = old == Err(sheets::SheetError::OldExcel);
        ok &= refused;
        say(&format!(
            "xls {}",
            if refused {
                "refused with words"
            } else {
                "NOT REFUSED"
            }
        ));

        say(if ok { "sheets=ok" } else { "sheets=FAILED" });
        if ok {
            0
        } else {
            1
        }
    }
}

krate::export!(Component);
