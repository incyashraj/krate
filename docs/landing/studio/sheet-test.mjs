// "Start from a spreadsheet" (G6): docs/landing/kit/sheet.js as it ships.
//
// CSV (quotes, separators, line breaks inside a field, a BOM, unicode,
// empty rows), XLSX (inline and shared strings, rich text, dates by
// built-in and custom formats, the 1904 calendar, booleans, formulas,
// gaps, several sheets), the kind of value in each column, the 5 MB and
// 20,000-row caps with their words, and the request a build starts from:
// the columns and a sample of rows, never the whole sheet, which travels
// beside it as the app's asset.
//
//   node docs/landing/studio/sheet-test.mjs
import { readFileSync } from "node:fs";
import { deflateRawSync } from "node:zlib";
import "../kit/sheet.js";

const S = globalThis.KrateSheet;
let bad = 0;
function check(ok, name) {
  if (!ok) bad++;
  console.log(`${ok ? "ok  " : "FAIL"} ${name}`);
}
const enc = new TextEncoder();
const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);

/* ---- CSV --------------------------------------------------------------- */
{
  const rows = S.parseCsv('Name,Note,Amount\r\n"Smith, Jo","He said ""hi""",12.5\r\n"Two\nlines",,3\n');
  check(same(rows, [["Name", "Note", "Amount"], ["Smith, Jo", 'He said "hi"', "12.5"], ["Two\nlines", "", "3"]]), "CSV: quoted separators, doubled quotes, a line break inside a field, CRLF and LF");
  check(same(S.parseCsv("﻿a;b\n1;2"), [["a", "b"], ["1", "2"]]), "CSV: a BOM is dropped and a semicolon sheet is read as one");
  check(same(S.parseCsv("a\tb\n1\t2", "\t"), [["a", "b"], ["1", "2"]]), "TSV: tabs");
  check(same(S.parseCsv("x,y\r1,2\r"), [["x", "y"], ["1", "2"]]), "CSV: old Mac line ends");
  check(same(S.parseCsv("a,b\n1,2"), [["a", "b"], ["1", "2"]]), "CSV: no line break at the end");
  const t = S.table(S.parseCsv("Café,番茄,Zoë\n\n,,\nä,ö,ü\n\n"));
  check(same(t.columns.map((c) => c.name), ["Café", "番茄", "Zoë"]) && same(t.rows, [["ä", "ö", "ü"]]), "unicode survives, and empty rows (and rows of empty cells) are dropped");
}

/* ---- the header -------------------------------------------------------- */
{
  const t = S.table([["Amount", "", "Amount", " Note "], ["1", "x", "2", "n"], ["3", "", "4", ""]]);
  check(same(t.columns.map((c) => c.name), ["Amount", "Column 2", "Amount 2", "Note"]), "a column with no name is named, a repeated name is made unique, spaces trimmed");
  const u = S.table([["A", "", "B", ""], ["1", "", "2"]]);
  check(same(u.columns.map((c) => c.name), ["A", "B"]) && same(u.rows, [["1", "2"]]), "a column with no name and nothing in it is dropped");
  const w = S.table([["A", "B", "C"], ["1"]]);
  check(same(w.rows, [["1", "", ""]]), "a short row is padded to the header");
}

/* ---- kinds of value ------------------------------------------------------ */
{
  const kind = (vals) => S.inferType(vals);
  check(kind(["12", "-3.5", "1,234.50", "", "0"]) === "number", "numbers, with thousands and blanks");
  check(kind(["$12.00", "£3", "4.50"]) === "money", "money");
  check(kind(["12%", "3.5 %"]) === "percent", "percentages");
  check(kind(["2026-03-14", "14/03/2026", "2026/3/1"]) === "date", "dates");
  check(kind(["09:30", "5:15 pm"]) === "time", "times");
  check(kind(["yes", "No", "TRUE", "false"]) === "yes/no", "yes/no");
  check(kind(["a@b.co", "x.y@corp.example"]) === "email", "email addresses");
  check(kind(["12", "twelve"]) === "text", "a mix is text");
  check(kind(["", " "]) === "empty", "nothing in it is empty");
  check(kind(["-", "."]) === "text", "a lone dash or dot is not a number");
}

/* ---- XLSX: a file made by openpyxl (inline strings, styles) ------------ */
{
  const bytes = new Uint8Array(readFileSync("docs/landing/studio/sheet-fixture.xlsx"));
  const got = await S.read("Stock list.xlsx", bytes);
  check(same(got.sheets.map((s) => s.name), ["Stock", "Empty", "People"]), "every sheet, by name, in the workbook's order");
  const t = S.table(got.sheets[0].rows);
  check(same(t.columns.map((c) => c.name), ["Item", "Qty", "Price", "Received", "In stock", "Supplier email", "Note", "Total"]), "the header row");
  check(t.rows.length === 3, "three rows: the empty row in the middle is dropped");
  check(same(t.rows[0].slice(0, 6), ["Café crème", "12", "3.5", "2026-03-14", "TRUE", "anna@example.com"]), "text with accents, numbers, a date by its style, a boolean");
  check(t.rows[1][0] === "番茄 (tomato)" && t.rows[1][6] === 'Said "fresh", ok', "CJK text and quotes");
  check(t.rows[0][6] === "Line one\nline two", "a line break inside a cell");
  check(t.rows[2][6] === "", "an empty cell is empty");
  check(same(t.columns.map((c) => c.type), ["text", "number", "number", "date", "yes/no", "email", "text", "empty"]), "each column's kind: " + t.columns.map((c) => c.type).join(", "));
  const people = S.table(got.sheets[2].rows);
  check(people.rows[0][1] === "2026-05-01 09:30", "a date with a time keeps its time");
  check(S.table(got.sheets[1].rows).rows.length === 0, "an empty sheet has no rows");
}

/* ---- XLSX: a file the way Excel writes it ------------------------------ */
function zip(files) {
  const parts = [];
  const central = [];
  let at = 0;
  for (const [name, text, stored] of files) {
    const raw = enc.encode(text);
    const data = stored ? raw : deflateRawSync(raw);
    const nm = enc.encode(name);
    const local = new Uint8Array(30 + nm.length);
    const dv = new DataView(local.buffer);
    dv.setUint32(0, 0x04034b50, true); dv.setUint16(8, stored ? 0 : 8, true);
    dv.setUint32(18, data.length, true); dv.setUint32(22, raw.length, true); dv.setUint16(26, nm.length, true);
    local.set(nm, 30);
    const cen = new Uint8Array(46 + nm.length);
    const cv = new DataView(cen.buffer);
    cv.setUint32(0, 0x02014b50, true); cv.setUint16(10, stored ? 0 : 8, true);
    cv.setUint32(20, data.length, true); cv.setUint32(24, raw.length, true); cv.setUint16(28, nm.length, true); cv.setUint32(42, at, true);
    cen.set(nm, 46);
    parts.push(local, data);
    central.push(cen);
    at += local.length + data.length;
  }
  const size = central.reduce((s, c) => s + c.length, 0);
  const end = new Uint8Array(22);
  const ev = new DataView(end.buffer);
  ev.setUint32(0, 0x06054b50, true); ev.setUint16(8, central.length, true); ev.setUint16(10, central.length, true);
  ev.setUint32(12, size, true); ev.setUint32(16, at, true);
  const all = [...parts, ...central, end];
  const out = new Uint8Array(all.reduce((s, p) => s + p.length, 0));
  let o = 0;
  for (const p of all) { out.set(p, o); o += p.length; }
  return out;
}
function excelBook({ date1904 = false, rows }) {
  return zip([
    ["xl/workbook.xml", `<?xml version="1.0"?><workbook xmlns:r="r"><workbookPr${date1904 ? ' date1904="1"' : ""}/><sheets><sheet name="Orders &amp; returns" sheetId="1" r:id="rId7"/></sheets></workbook>`],
    ["xl/_rels/workbook.xml.rels", `<Relationships><Relationship Id="rId7" Type="x" Target="worksheets/orders.xml"/></Relationships>`],
    ["xl/sharedStrings.xml", `<sst><si><t>Order</t></si><si><t>Placed</t></si><si><r><t>Cust</t></r><r><rPr/><t xml:space="preserve">omer</t></r><rPh><t>X</t></rPh></si><si><t>Ünïcødé &amp; co</t></si><si><t>Paid</t></si><si/></sst>`],
    ["xl/styles.xml", `<styleSheet><numFmts count="1"><numFmt numFmtId="164" formatCode="dd/mm/yyyy;@"/><numFmt numFmtId="165" formatCode="&quot;Qty&quot; 0"/></numFmts><cellXfs count="3"><xf numFmtId="0"/><xf numFmtId="164"/><xf numFmtId="165"/></cellXfs></styleSheet>`],
    ["xl/worksheets/orders.xml", `<worksheet><sheetData>${rows}</sheetData></worksheet>`, true],
  ]);
}
{
  const rows = `<row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="s"><v>1</v></c><c r="C1" t="s"><v>2</v></c><c r="D1" t="s"><v>4</v></c></row>`
    + `<row r="2"><c r="A2"><v>1001</v></c><c r="B2" s="1"><v>46095</v></c><c r="C2" t="s"><v>3</v></c><c r="D2" t="b"><v>1</v></c></row>`
    + `<row r="4"><c r="A4" s="2"><v>1002</v></c><c r="C4" t="str"><v>formula text</v></c><c r="D4" t="e"><v>#N/A</v></c></row>`;
  const got = await S.read("orders.xlsx", excelBook({ rows }));
  check(got.sheets[0].name === "Orders & returns", "a sheet name with an entity, found through the workbook's own relationship");
  const t = S.table(got.sheets[0].rows);
  check(same(t.columns.map((c) => c.name), ["Order", "Placed", "Customer", "Paid"]), "shared strings, and rich text runs joined (the phonetic hint is not the text)");
  check(same(t.rows[0], ["1001", "2026-03-14", "Ünïcødé & co", "TRUE"]), "a custom date format is a date; unicode in a shared string");
  check(same(t.rows[1], ["1002", "", "formula text", "#N/A"]), "a number format that is not a date stays a number; a gap in the row is empty; formula text and errors as shown");
  const old = S.table((await S.read("old.xlsx", excelBook({ date1904: true, rows: `<row r="1"><c r="A1" t="s"><v>1</v></c></row><row r="2"><c r="A2" s="1"><v>44633</v></c></row>` }))).sheets[0].rows);
  check(old.rows[0][0] === "2026-03-14", "the 1904 calendar");
}

/* ---- refused, in words that say what to do ------------------------------ */
{
  const words = async (name, bytes) => { try { await S.read(name, bytes); return null; } catch (e) { return e; } };
  const big = new Uint8Array(S.MAX_BYTES + 1);
  const e1 = await words("Everything.csv", big);
  check(e1 && e1.reason === "too-large" && /Everything\.csv is 5 MB\. Krate takes a spreadsheet up to 5 MB: keep only the sheet and the columns you need/.test(e1.message), "over 5 MB: refused before it is read, by name and size: " + (e1 && e1.message));
  const atCap = new Uint8Array(S.MAX_BYTES).fill(0x61);
  check((await words("ok.csv", atCap)) === null, "exactly 5 MB is taken");
  const e2 = await words("Old.xls", enc.encode("x"));
  check(e2 && e2.reason === "wrong-kind" && /save it as \.xlsx or \.csv/.test(e2.message), "an old .xls says how to make it one Krate reads");
  const e3 = await words("notes.pdf", enc.encode("x"));
  check(e3 && /Choose a \.csv or \.xlsx file/.test(e3.message), "another kind of file is refused");
  const e4 = await words("broken.xlsx", enc.encode("not a zip at all"));
  check(e4 && e4.reason === "unreadable" && /save it again as \.xlsx or \.csv/.test(e4.message), "a broken .xlsx says how to fix it");
  const e5 = await words("blank.csv", new Uint8Array(0));
  check(e5 && e5.reason === "empty", "an empty file");
  check(S.check(S.table([["Only a header"]]), "head.csv").reason === "empty", "a header with nothing under it is refused");
  const many = [["n"]];
  for (let i = 0; i < S.MAX_ROWS + 1; i++) many.push([String(i)]);
  const e6 = S.check(S.table(many), "huge.csv");
  check(e6 && e6.reason === "too-many-rows" && /huge\.csv has 20,001 rows\. Krate takes up to 20,000/.test(e6.message), "past 20,000 rows: " + (e6 && e6.message));
  check(S.check(S.table(many.slice(0, S.MAX_ROWS + 1)), "ok.csv") === null, "20,000 rows are taken");
  // An .xlsx past the cap stops reading early and says "more than".
  let xr = `<row r="1"><c r="A1" t="s"><v>0</v></c></row>`;
  for (let i = 2; i < S.MAX_ROWS + 1100; i++) xr += `<row r="${i}"><c r="A${i}"><v>${i}</v></c></row>`;
  const longBook = S.table((await S.read("long.xlsx", excelBook({ rows: xr }))).sheets[0].rows);
  const e7 = S.check(longBook, "long.xlsx");
  check(e7 && /long\.xlsx has more than 20,000 rows/.test(e7.message), "an .xlsx past the cap: " + (e7 && e7.message));
}

/* ---- the request a build starts from ------------------------------------ */
{
  const head = ["Item", "Qty", "Price", "Received", "Supplier"];
  const rows = [head];
  for (let i = 1; i <= 3000; i++) rows.push([`Part ${i}, size ${i % 7}`, String(i), (i / 4).toFixed(2), "2026-03-14", `s${i}@example.com`]);
  const t = S.table(rows);
  const out = S.request({ app: "Stock", purpose: "Track what the warehouse holds and reorder in time.", org: { slug: "acme", name: "Acme" }, table: t, file: "Warehouse stock (final).xlsx" });
  const r = out.request;
  check(r.length <= 2000, `the request fits the build service's 2,000 characters (${r.length})`);
  check(r.startsWith("Stock: Track what the warehouse holds and reorder in time."), "it starts with the name and what it is for");
  check(/3,000 rows and 5 columns: Item \(text\), Qty \(number\), Price \(number\), Received \(date\), Supplier \(email\)\./.test(r), "the columns with their kinds, and how many rows");
  check(r.includes("Part 1, size 1 | 1 | 0.25 | 2026-03-14 | s1@example.com") && r.includes("Part 5, size 5"), "a sample of the first rows");
  check(!r.includes("Part 6, size 6") && !r.includes("Part 2999"), "and no more than the sample: the rest of the sheet is not in the request");
  check(out.sampleRows === 5, "five sample rows");
  check(/Acme's team data with store\.org/.test(r) && /everyone in Acme sees the same records/.test(r), "made for an organisation: the records are its team data");
  check(/attached as attached\/warehouse-stock-final\.csv\. Copy it unchanged into the app's assets\/ folder and do not read all of it/.test(r), "the whole sheet is the attached file, copied into the app, not read");
  check(/once a sync has reached Acme \(org::who\(\) answers\) and there are no records yet, read it with krate::assets::read_text\("warehouse-stock-final\.csv"\)/.test(r) && /row number as the id, so two people importing at once write the same records/.test(r), "the team's first run imports the bundled asset, once for everyone");
  check(out.asset.name === "warehouse-stock-final.csv", "the asset is named after the sheet");
  const lines = out.asset.text.trim().split("\n");
  check(lines.length === 3001 && lines[0] === "Item,Qty,Price,Received,Supplier" && lines[1] === '"Part 1, size 1",1,0.25,2026-03-14,s1@example.com', "the asset is the whole sheet as CSV, quoted where it must be");
  check(S.parseCsv(out.asset.text).length === 3001, "and it reads back row for row");
  const mine = S.request({ app: "Stock", purpose: "", org: null, table: t, file: "stock.csv" }).request;
  check(/on this computer with store\.sql/.test(mine) && !/store\.org|org::who/.test(mine) && /when there are no records yet, read it with krate::assets::read_text\("stock\.csv"\)/.test(mine), "made for one person: the records stay on the computer, imported on first run");
  // Wide and wordy sheets still fit, with fewer columns named.
  const wide = [Array.from({ length: 60 }, (_, i) => `A rather long column heading number ${i}`)];
  for (let i = 0; i < 10; i++) wide.push(Array.from({ length: 60 }, () => "x".repeat(200)));
  const w = S.request({ app: "W".repeat(200), purpose: "p".repeat(900), org: null, table: S.table(wide), file: "wide.csv" });
  check(w.request.length <= 2000 && /and \d+ more\./.test(w.request) && /store\.sql/.test(w.request), `a very wide sheet still fits (${w.request.length}) and still says how to keep the records`);
  check(S.assetName("番茄.xlsx") === "records.csv" && S.assetName("Café list.csv") === "cafe-list.csv", "asset names are plain letters, digits and dashes");
}

if (bad) {
  console.log(`\n${bad} check(s) failed`);
  process.exit(1);
}
console.log("\nall spreadsheet checks passed");
