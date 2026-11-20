//! Office Open XML workbooks (.xlsx, .xlsm): read, and write.

use super::xml::{self, attr, attr_text, local, push_escaped, push_unescaped, Event};
use super::zip::{Archive, Writer};
use super::{
    clean_sheet_name, column_index, number_exact, Cell, DateTime, Sheet, SheetError, Workbook,
};
use alloc::{collections::BTreeMap, string::String, vec::Vec};
use core::fmt::Write as _;

/// Excel's own limits; a cell reference past them is damage, not data.
const MAX_ROWS: usize = 1_048_576;
const MAX_COLS: usize = 16_384;

pub(crate) fn looks_like(zip: &Archive<'_>) -> bool {
    zip.has("[Content_Types].xml") && (zip.has("_rels/.rels") || zip.has("xl/workbook.xml"))
}

/// The directory part of a part name: `xl/workbook.xml` -> `xl/`.
fn dir_of(path: &str) -> &str {
    match path.rfind('/') {
        Some(at) => path.get(..=at).unwrap_or(""),
        None => "",
    }
}

/// Resolve a relationship target against the part that names it.
fn resolve(base_dir: &str, target: &str) -> String {
    let joined = if let Some(abs) = target.strip_prefix('/') {
        String::from(abs)
    } else {
        let mut s = String::from(base_dir);
        s.push_str(target);
        s
    };
    let mut parts: Vec<&str> = Vec::new();
    for seg in joined.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    parts.join("/")
}

struct Rel {
    id: String,
    kind: String,
    target: String,
}

/// The relationships of `part`, with targets resolved to part names.
fn rels_of(zip: &Archive<'_>, part: &str) -> Result<Vec<Rel>, SheetError> {
    let dir = dir_of(part);
    let file = part.get(dir.len()..).unwrap_or("");
    let mut rels_path = String::from(dir);
    rels_path.push_str("_rels/");
    rels_path.push_str(file);
    rels_path.push_str(".rels");
    let Some(text) = zip.read_text(&rels_path)? else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for ev in xml::Reader::new(&text) {
        if let Event::Start { name, attrs, .. } = ev {
            if local(name) == "Relationship" {
                let external = attr(attrs, "TargetMode") == Some("External");
                let target = attr_text(attrs, "Target").unwrap_or_default();
                out.push(Rel {
                    id: attr_text(attrs, "Id").unwrap_or_default(),
                    kind: attr_text(attrs, "Type").unwrap_or_default(),
                    target: if external {
                        target
                    } else {
                        resolve(dir, &target)
                    },
                });
            }
        }
    }
    Ok(out)
}

fn rel_of_kind<'r>(rels: &'r [Rel], kind: &str) -> Option<&'r Rel> {
    rels.iter().find(|r| r.kind.ends_with(kind))
}

pub(crate) fn read(zip: &Archive<'_>) -> Result<Workbook, SheetError> {
    let root = rels_of(zip, "")?;
    let book_path = rel_of_kind(&root, "/officeDocument")
        .map(|r| r.target.clone())
        .unwrap_or_else(|| String::from("xl/workbook.xml"));
    let book_text = zip.read_text(&book_path)?.ok_or(SheetError::Damaged)?;
    let rels = rels_of(zip, &book_path)?;
    let dir = dir_of(&book_path);

    let shared = match rel_of_kind(&rels, "/sharedStrings") {
        Some(r) => zip.read_text(&r.target)?,
        None => zip.read_text(&resolve(dir, "sharedStrings.xml"))?,
    };
    let shared = shared.map(|t| shared_strings(&t)).unwrap_or_default();
    let styles = match rel_of_kind(&rels, "/styles") {
        Some(r) => zip.read_text(&r.target)?,
        None => zip.read_text(&resolve(dir, "styles.xml"))?,
    };
    let date_styles = styles.map(|t| date_styles(&t)).unwrap_or_default();

    let mut date1904 = false;
    let mut listed: Vec<(String, String)> = Vec::new();
    for ev in xml::Reader::new(&book_text) {
        if let Event::Start { name, attrs, .. } = ev {
            match local(name) {
                "workbookPr" => {
                    date1904 = matches!(attr(attrs, "date1904"), Some("1" | "true"));
                }
                "sheet" => listed.push((
                    attr_text(attrs, "name").unwrap_or_default(),
                    attr_text(attrs, "id").unwrap_or_default(),
                )),
                _ => {}
            }
        }
    }

    let ctx = Context {
        shared: &shared,
        date_styles: &date_styles,
        date1904,
    };
    let mut book = Workbook::new();
    for (name, id) in listed {
        let Some(rel) = rels.iter().find(|r| r.id == id) else {
            continue;
        };
        // Chart sheets and dialog sheets have no cells.
        if !rel.kind.is_empty() && !rel.kind.ends_with("/worksheet") {
            continue;
        }
        let Some(text) = zip.read_text(&rel.target)? else {
            continue;
        };
        let mut sheet = parse_sheet(&text, &ctx);
        sheet.name = name;
        book.sheets.push(sheet);
    }
    Ok(book)
}

/// Excel escapes characters XML cannot hold as `_xHHHH_`.
fn push_ooxml_text(out: &mut String, raw: &str) {
    let start = out.len();
    push_unescaped(out, raw);
    if !out.get(start..).unwrap_or("").contains("_x") {
        return;
    }
    let text = String::from(out.get(start..).unwrap_or(""));
    out.truncate(start);
    let mut rest = text.as_str();
    while let Some(at) = rest.find("_x") {
        out.push_str(rest.get(..at).unwrap_or(""));
        let tail = rest.get(at..).unwrap_or("");
        let code = tail
            .get(2..6)
            .filter(|_| tail.get(6..7) == Some("_"))
            .and_then(|hex| u32::from_str_radix(hex, 16).ok())
            .and_then(char::from_u32);
        match code {
            Some(c) => {
                out.push(c);
                rest = tail.get(7..).unwrap_or("");
            }
            None => {
                out.push_str("_x");
                rest = tail.get(2..).unwrap_or("");
            }
        }
    }
    out.push_str(rest);
}

/// The shared string table: each `<si>` is one string, made of its `<t>`
/// runs; phonetic guides (`<rPh>`) are not part of the text.
fn shared_strings(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current: Option<String> = None;
    let mut in_t = false;
    let mut in_rph = 0u32;
    for ev in xml::Reader::new(text) {
        match ev {
            Event::Start { name, empty, .. } => match local(name) {
                "si" if empty => out.push(String::new()),
                "si" => current = Some(String::new()),
                "t" if !empty => in_t = true,
                "rPh" if !empty => in_rph += 1,
                _ => {}
            },
            Event::End { name } => match local(name) {
                "si" => out.push(current.take().unwrap_or_default()),
                "t" => in_t = false,
                "rPh" => in_rph = in_rph.saturating_sub(1),
                _ => {}
            },
            Event::Text(t) if in_t && in_rph == 0 => {
                if let Some(s) = current.as_mut() {
                    push_ooxml_text(s, t);
                }
            }
            Event::CData(t) if in_t && in_rph == 0 => {
                if let Some(s) = current.as_mut() {
                    s.push_str(t);
                }
            }
            _ => {}
        }
    }
    out
}

/// For each cell style (`<cellXfs>` entry, by index): does its number
/// format show a date or a time?
fn date_styles(text: &str) -> Vec<bool> {
    let mut custom: BTreeMap<u32, bool> = BTreeMap::new();
    let mut out = Vec::new();
    let mut in_cell_xfs = false;
    for ev in xml::Reader::new(text) {
        match ev {
            Event::Start { name, attrs, empty } => match local(name) {
                "numFmt" => {
                    let id = attr(attrs, "numFmtId").and_then(|v| v.parse().ok());
                    let code = attr_text(attrs, "formatCode").unwrap_or_default();
                    if let Some(id) = id {
                        custom.insert(id, format_is_date(&code));
                    }
                }
                "cellXfs" if !empty => in_cell_xfs = true,
                "xf" if in_cell_xfs => {
                    let id: u32 = attr(attrs, "numFmtId")
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(0);
                    let is_date = custom
                        .get(&id)
                        .copied()
                        .unwrap_or_else(|| builtin_is_date(id));
                    out.push(is_date);
                }
                _ => {}
            },
            Event::End { name } if local(name) == "cellXfs" => in_cell_xfs = false,
            _ => {}
        }
    }
    out
}

/// Excel's built-in number formats that show a date or a time.
fn builtin_is_date(id: u32) -> bool {
    matches!(id, 14..=22 | 27..=36 | 45..=47 | 50..=58)
}

/// Does a format code show a date or time? Looks for d, m, y, h or s
/// outside quoted text, escapes, fills, padding and `[colour]` sections.
pub(crate) fn format_is_date(code: &str) -> bool {
    let section = code.split(';').next().unwrap_or("");
    let mut chars = section.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                for q in chars.by_ref() {
                    if q == '"' {
                        break;
                    }
                }
            }
            '\\' | '_' | '*' => {
                chars.next();
            }
            '[' => {
                let mut inner = String::new();
                for q in chars.by_ref() {
                    if q == ']' {
                        break;
                    }
                    inner.push(q);
                }
                // [h], [mm], [ss]: elapsed time.
                let t = inner.to_ascii_lowercase();
                if !t.is_empty() && t.chars().all(|c| matches!(c, 'h' | 'm' | 's')) {
                    return true;
                }
            }
            'd' | 'D' | 'm' | 'M' | 'y' | 'Y' | 'h' | 'H' | 's' | 'S' => return true,
            _ => {}
        }
    }
    false
}

struct Context<'a> {
    shared: &'a [String],
    date_styles: &'a [bool],
    date1904: bool,
}

/// "AB12" -> (row 11, col 27). Either part may be missing.
fn split_ref(r: &str) -> (Option<usize>, Option<usize>) {
    let r = r.trim().trim_start_matches('$');
    let split = r.find(|c: char| c.is_ascii_digit()).unwrap_or(r.len());
    let letters = r.get(..split).unwrap_or("").trim_end_matches('$');
    let digits = r.get(split..).unwrap_or("");
    let col = column_index(letters);
    let row = digits.parse::<usize>().ok().and_then(|n| n.checked_sub(1));
    (row, col)
}

fn parse_sheet(text: &str, ctx: &Context<'_>) -> Sheet {
    let mut sheet = Sheet::default();
    let mut in_data = false;
    let mut row: usize = 0;
    let mut next_row: usize = 0;
    let mut col: usize = 0;
    let mut kind = String::new();
    let mut style: usize = 0;
    let mut value = String::new();
    let mut in_cell = false;
    let mut in_value = false;
    let mut in_rph = 0u32;
    let mut merges: Vec<String> = Vec::new();
    for ev in xml::Reader::new(text) {
        match ev {
            Event::Start { name, attrs, empty } => match local(name) {
                "sheetData" if !empty => in_data = true,
                "row" if in_data => {
                    row = attr(attrs, "r")
                        .and_then(|v| v.trim().parse::<usize>().ok())
                        .and_then(|n| n.checked_sub(1))
                        .unwrap_or(next_row);
                    next_row = row + 1;
                    col = 0;
                }
                "c" if in_data => {
                    let (r, c) = attr(attrs, "r").map(split_ref).unwrap_or((None, None));
                    if let Some(r) = r {
                        row = r;
                        next_row = r + 1;
                    }
                    if let Some(c) = c {
                        col = c;
                    }
                    kind.clear();
                    kind.push_str(attr(attrs, "t").unwrap_or("n"));
                    style = attr(attrs, "s").and_then(|v| v.parse().ok()).unwrap_or(0);
                    value.clear();
                    in_cell = !empty;
                    if empty {
                        col += 1;
                    }
                }
                "v" | "t" if in_cell && !empty => in_value = true,
                "rPh" if in_cell && !empty => in_rph += 1,
                "mergeCell" => {
                    if let Some(range) = attr(attrs, "ref") {
                        merges.push(String::from(range));
                    }
                }
                _ => {}
            },
            Event::End { name } => match local(name) {
                "sheetData" => in_data = false,
                "c" if in_cell => {
                    let cell = make_cell(&kind, style, &value, ctx);
                    if !matches!(cell, Cell::Empty) && row < MAX_ROWS && col < MAX_COLS {
                        sheet.set(row, col, cell);
                    }
                    in_cell = false;
                    col += 1;
                }
                "v" | "t" => in_value = false,
                "rPh" => in_rph = in_rph.saturating_sub(1),
                _ => {}
            },
            Event::Text(t) if in_value && in_rph == 0 => push_ooxml_text(&mut value, t),
            Event::CData(t) if in_value && in_rph == 0 => value.push_str(t),
            _ => {}
        }
    }
    clear_merged(&mut sheet, &merges);
    sheet
}

/// A merged range shows only its top-left cell. Excel can leave old values
/// under the rest of the range; the person never sees them, so neither
/// does the app.
fn clear_merged(sheet: &mut Sheet, merges: &[String]) {
    for range in merges {
        let mut ends = range.split(':');
        let (Some(a), Some(b)) = (ends.next(), ends.next()) else {
            continue;
        };
        let ((Some(r0), Some(c0)), (Some(r1), Some(c1))) = (split_ref(a), split_ref(b)) else {
            continue;
        };
        let last_row = r1.min(sheet.rows.len().saturating_sub(1));
        for r in r0..=last_row {
            let Some(row) = sheet.rows.get_mut(r) else {
                break;
            };
            let last_col = c1.min(row.len().saturating_sub(1));
            for c in c0..=last_col {
                if (r, c) != (r0, c0) {
                    if let Some(cell) = row.get_mut(c) {
                        *cell = Cell::Empty;
                    }
                }
            }
            while matches!(row.last(), Some(Cell::Empty)) {
                row.pop();
            }
        }
    }
    while sheet.rows.last().is_some_and(Vec::is_empty) {
        sheet.rows.pop();
    }
}

fn make_cell(kind: &str, style: usize, value: &str, ctx: &Context<'_>) -> Cell {
    match kind {
        "s" => value
            .trim()
            .parse::<usize>()
            .ok()
            .and_then(|i| ctx.shared.get(i))
            .map(|s| Cell::Text(s.clone()))
            .unwrap_or(Cell::Empty),
        "inlineStr" | "str" => {
            if value.is_empty() {
                Cell::Empty
            } else {
                Cell::Text(String::from(value))
            }
        }
        "b" => Cell::Bool(matches!(value.trim(), "1" | "true" | "TRUE")),
        "e" => Cell::Error(String::from(value.trim())),
        "d" => DateTime::parse_iso(value)
            .map(Cell::Date)
            .unwrap_or_else(|| Cell::Text(String::from(value))),
        _ => {
            let v = value.trim();
            if v.is_empty() {
                return Cell::Empty;
            }
            match v.parse::<f64>() {
                Ok(n) if ctx.date_styles.get(style).copied().unwrap_or(false) => {
                    Cell::Date(DateTime::from_serial_in(n, ctx.date1904))
                }
                Ok(n) => Cell::Number(n),
                Err(_) => Cell::Text(String::from(v)),
            }
        }
    }
}

/// Write a workbook as a standard `.xlsx` file (SpreadsheetML: a shared
/// string table, one part per sheet, a minimal style sheet).
///
/// Text, numbers, TRUE/FALSE and errors keep their type; dates are real
/// dates (shown `yyyy-mm-dd`, or `yyyy-mm-dd hh:mm:ss` with a time, or
/// `hh:mm:ss` for a time alone). Sheet names are made acceptable to Excel
/// (31 characters, no `[]:*?/\`, unique), and a workbook with no sheets
/// gets one empty "Sheet1", because Excel will not open a workbook with
/// none. The same workbook always gives the same bytes.
pub fn write_xlsx(book: &Workbook) -> Vec<u8> {
    let empty = [Sheet::new("Sheet1")];
    let sheets: &[Sheet] = if book.sheets.is_empty() {
        &empty
    } else {
        &book.sheets
    };
    let mut names: Vec<String> = Vec::new();
    for s in sheets {
        let n = clean_sheet_name(&s.name, &names);
        names.push(n);
    }

    let mut strings = Strings::default();
    let mut sheet_xml = Vec::new();
    for s in sheets {
        sheet_xml.push(sheet_part(s, &mut strings));
    }

    let mut zip = Writer::new();
    let mut types = String::from(concat!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
        "\n",
        r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">"#,
        r#"<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>"#,
        r#"<Default Extension="xml" ContentType="application/xml"/>"#,
        r#"<Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>"#,
        r#"<Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/>"#,
        r#"<Override PartName="/xl/sharedStrings.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml"/>"#,
    ));
    for i in 1..=sheets.len() {
        let _ = write!(
            types,
            r#"<Override PartName="/xl/worksheets/sheet{i}.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>"#
        );
    }
    types.push_str("</Types>");
    zip.add("[Content_Types].xml", types.as_bytes());

    zip.add(
        "_rels/.rels",
        concat!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
            "\n",
            r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
            r#"<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>"#,
            r#"</Relationships>"#
        )
        .as_bytes(),
    );

    let mut workbook = String::from(concat!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
        "\n",
        r#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">"#,
        r#"<bookViews><workbookView/></bookViews><sheets>"#,
    ));
    let mut book_rels = String::from(concat!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
        "\n",
        r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
    ));
    for (i, name) in names.iter().enumerate() {
        let n = i + 1;
        workbook.push_str(r#"<sheet name=""#);
        push_escaped(&mut workbook, name);
        let _ = write!(workbook, r#"" sheetId="{n}" r:id="rId{n}"/>"#);
        let _ = write!(
            book_rels,
            r#"<Relationship Id="rId{n}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet{n}.xml"/>"#
        );
    }
    workbook.push_str("</sheets></workbook>");
    let next = sheets.len() + 1;
    let _ = write!(
        book_rels,
        r#"<Relationship Id="rId{next}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>"#
    );
    let _ = write!(
        book_rels,
        r#"<Relationship Id="rId{}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings" Target="sharedStrings.xml"/>"#,
        next + 1
    );
    book_rels.push_str("</Relationships>");
    zip.add("xl/workbook.xml", workbook.as_bytes());
    zip.add("xl/_rels/workbook.xml.rels", book_rels.as_bytes());
    zip.add("xl/styles.xml", STYLES.as_bytes());

    let mut sst = String::from(concat!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
        "\n",
    ));
    let _ = write!(
        sst,
        r#"<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" count="{}" uniqueCount="{}">"#,
        strings.uses,
        strings.list.len()
    );
    for s in &strings.list {
        if s.starts_with([' ', '\t', '\n']) || s.ends_with([' ', '\t', '\n']) {
            sst.push_str(r#"<si><t xml:space="preserve">"#);
        } else {
            sst.push_str("<si><t>");
        }
        push_escaped(&mut sst, s);
        sst.push_str("</t></si>");
    }
    sst.push_str("</sst>");
    zip.add("xl/sharedStrings.xml", sst.as_bytes());

    for (i, xml) in sheet_xml.iter().enumerate() {
        let mut name = String::new();
        let _ = write!(name, "xl/worksheets/sheet{}.xml", i + 1);
        zip.add(&name, xml.as_bytes());
    }
    zip.finish()
}

/// Cell styles: 0 general, 1 a date, 2 a date and time, 3 a time.
const STYLES: &str = concat!(
    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
    "\n",
    r#"<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">"#,
    r#"<numFmts count="3"><numFmt numFmtId="164" formatCode="yyyy\-mm\-dd"/><numFmt numFmtId="165" formatCode="yyyy\-mm\-dd\ hh:mm:ss"/><numFmt numFmtId="166" formatCode="hh:mm:ss"/></numFmts>"#,
    r#"<fonts count="1"><font><sz val="11"/><name val="Calibri"/><family val="2"/></font></fonts>"#,
    r#"<fills count="2"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill></fills>"#,
    r#"<borders count="1"><border><left/><right/><top/><bottom/><diagonal/></border></borders>"#,
    r#"<cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs>"#,
    r#"<cellXfs count="4"><xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/>"#,
    r#"<xf numFmtId="164" fontId="0" fillId="0" borderId="0" xfId="0" applyNumberFormat="1"/>"#,
    r#"<xf numFmtId="165" fontId="0" fillId="0" borderId="0" xfId="0" applyNumberFormat="1"/>"#,
    r#"<xf numFmtId="166" fontId="0" fillId="0" borderId="0" xfId="0" applyNumberFormat="1"/></cellXfs>"#,
    r#"<cellStyles count="1"><cellStyle name="Normal" xfId="0" builtinId="0"/></cellStyles>"#,
    r#"</styleSheet>"#
);

#[derive(Default)]
struct Strings {
    list: Vec<String>,
    index: BTreeMap<String, usize>,
    uses: usize,
}

impl Strings {
    fn id(&mut self, s: &str) -> usize {
        self.uses += 1;
        if let Some(&i) = self.index.get(s) {
            return i;
        }
        let i = self.list.len();
        self.list.push(String::from(s));
        self.index.insert(String::from(s), i);
        i
    }
}

/// Excel shows at most 32,767 characters in a cell and refuses more.
fn clip(s: &str) -> &str {
    match s.char_indices().nth(32_767) {
        Some((at, _)) => s.get(..at).unwrap_or(s),
        None => s,
    }
}

fn sheet_part(sheet: &Sheet, strings: &mut Strings) -> String {
    let mut out = String::from(concat!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
        "\n",
        r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheetData>"#,
    ));
    for (r, row) in sheet.rows.iter().enumerate().take(MAX_ROWS) {
        if row.iter().all(|c| matches!(c, Cell::Empty)) {
            continue;
        }
        let _ = write!(out, r#"<row r="{}">"#, r + 1);
        for (c, cell) in row.iter().enumerate().take(MAX_COLS) {
            if matches!(cell, Cell::Empty) {
                continue;
            }
            out.push_str(r#"<c r=""#);
            out.push_str(&Sheet::column_name(c));
            let _ = write!(out, "{}\"", r + 1);
            match cell {
                Cell::Empty => {}
                Cell::Text(t) => {
                    let _ = write!(out, r#" t="s"><v>{}</v></c>"#, strings.id(clip(t)));
                }
                Cell::Number(n) if n.is_finite() => {
                    out.push_str("><v>");
                    out.push_str(&number_exact(*n));
                    out.push_str("</v></c>");
                }
                Cell::Number(_) => out.push_str(r#" t="e"><v>#NUM!</v></c>"#),
                Cell::Bool(b) => {
                    let _ = write!(out, r#" t="b"><v>{}</v></c>"#, u8::from(*b));
                }
                Cell::Date(d) => {
                    let has_time = d.hour != 0 || d.minute != 0 || d.second != 0;
                    let s = match (d.has_date(), has_time) {
                        (false, _) => 3,
                        (true, true) => 2,
                        (true, false) => 1,
                    };
                    let _ = write!(out, r#" s="{s}"><v>"#);
                    out.push_str(&number_exact(d.to_serial()));
                    out.push_str("</v></c>");
                }
                Cell::Error(e) => {
                    out.push_str(r#" t="e"><v>"#);
                    push_escaped(&mut out, e);
                    out.push_str("</v></c>");
                }
            }
        }
        out.push_str("</row>");
    }
    out.push_str("</sheetData></worksheet>");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_formats_are_recognised() {
        for code in [
            "yyyy-mm-dd",
            "d/m/yy",
            "[$-409]mmmm d, yyyy",
            "h:mm AM/PM",
            "[h]:mm",
            "dd\\.mm\\.yyyy",
        ] {
            assert!(format_is_date(code), "{code}");
        }
        for code in [
            "General",
            "0.00",
            "#,##0 [$CHF]",
            "\"days\" 0",
            "[Red]0.0",
            "0.00E+00",
            "_-* #,##0.00_-;-* #,##0.00_-",
        ] {
            assert!(!format_is_date(code), "{code}");
        }
    }

    #[test]
    fn references_split_into_row_and_column() {
        assert_eq!(split_ref("A1"), (Some(0), Some(0)));
        assert_eq!(split_ref("AB12"), (Some(11), Some(27)));
        assert_eq!(split_ref("$C$3"), (Some(2), Some(2)));
        assert_eq!(split_ref("XFD1048576"), (Some(1_048_575), Some(16_383)));
    }

    #[test]
    fn relationship_targets_resolve() {
        assert_eq!(
            resolve("xl/", "worksheets/sheet1.xml"),
            "xl/worksheets/sheet1.xml"
        );
        assert_eq!(
            resolve("xl/", "/xl/sharedStrings.xml"),
            "xl/sharedStrings.xml"
        );
        assert_eq!(resolve("xl/worksheets/", "../styles.xml"), "xl/styles.xml");
    }

    #[test]
    fn excel_escapes_decode() {
        let mut s = String::new();
        push_ooxml_text(&mut s, "a_x000D_b_xZZ_c");
        assert_eq!(s, "a\rb_xZZ_c");
    }
}
