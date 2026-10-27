/* window.KrateSheet -- a spreadsheet, read in this tab, made the start of
 * an app (G6, "turn this spreadsheet into an app").
 *
 *   read(name, bytes)      -> {file, sheets: [{name, rows}]}  (async)
 *                             .csv, .tsv or .xlsx; refused with .reason and
 *                             a sentence that says what to do next
 *   table(rows)            -> {columns: [{name, type}], rows}
 *                             the header row, the rows under it (empty rows
 *                             dropped), and each column's kind of value
 *   check(table, file)     -> null, or the refusal past the row cap
 *   toCsv(table)           -> the whole sheet as one plain CSV
 *   request({app, purpose, org, table, file})
 *                          -> {request, asset: {name, text}}
 *        the words a Studio build starts from -- what the app is for, the
 *        columns with their kinds and the first rows -- and the whole sheet
 *        as a file the build carries into the app's assets. The request
 *        holds a sample only; the rest of the sheet never goes into it.
 *
 * Nothing here talks to the network and nothing is loaded from anywhere:
 * the .xlsx is a zip of XML, opened with the browser's own
 * DecompressionStream and read with a few lines of string matching.
 * docs/landing/studio/sheet-test.mjs runs this file as it ships.
 */
(function (root) {
  "use strict";

  var MAX_BYTES = 5 * 1024 * 1024;
  var MAX_ROWS = 20000;
  /// The most an .xlsx may expand to: a 5 MB file of XML is far less, and
  /// a zip that claims more is not a spreadsheet anyone typed.
  var MAX_EXPANDED = 80 * 1024 * 1024;
  /// The longest request the build service works from (cloud/builder).
  var MAX_REQUEST = 2000;
  var SAMPLE_ROWS = 5;
  var CELL_CLIP = 40;

  function refuse(reason, message) {
    var e = new Error(message);
    e.reason = reason;
    return e;
  }
  function mb(n) { return (n / (1024 * 1024)).toFixed(1).replace(/\.0$/, "") + " MB"; }
  function count(n) { return Number(n).toLocaleString("en-US"); }

  // ------------------------------------------------------------------ CSV

  /// The separator a sheet uses: the one that appears most on its first
  /// line outside quotes (comma, semicolon or tab).
  function delimiterOf(text) {
    var counts = { ",": 0, ";": 0, "\t": 0 };
    var quoted = false;
    for (var i = 0; i < text.length; i++) {
      var c = text[i];
      if (c === '"') quoted = !quoted;
      else if (!quoted && (c === "\n" || c === "\r")) break;
      else if (!quoted && counts[c] !== undefined) counts[c]++;
    }
    var best = ",";
    if (counts[";"] > counts[best]) best = ";";
    if (counts["\t"] > counts[best]) best = "\t";
    return best;
  }

  /// CSV as rows of strings (RFC 4180): quoted fields hold separators,
  /// line breaks and "" for a quote; CRLF, LF or CR end a row.
  function parseCsv(text, delimiter) {
    text = String(text || "");
    if (text.charCodeAt(0) === 0xfeff) text = text.slice(1);
    var d = delimiter || delimiterOf(text);
    var rows = [];
    var row = [];
    var field = "";
    var quoted = false;
    var i = 0;
    var n = text.length;
    while (i < n) {
      var c = text[i];
      if (quoted) {
        if (c === '"') {
          if (text[i + 1] === '"') { field += '"'; i += 2; continue; }
          quoted = false; i++; continue;
        }
        field += c; i++; continue;
      }
      if (c === '"' && field === "") { quoted = true; i++; continue; }
      if (c === d) { row.push(field); field = ""; i++; continue; }
      if (c === "\r" || c === "\n") {
        row.push(field); rows.push(row); row = []; field = "";
        i += c === "\r" && text[i + 1] === "\n" ? 2 : 1;
        continue;
      }
      field += c; i++;
    }
    if (field !== "" || row.length) { row.push(field); rows.push(row); }
    return rows;
  }

  // ------------------------------------------------------------- the zip

  function u16(b, i) { return b[i] | (b[i + 1] << 8); }
  function u32(b, i) { return (b[i] | (b[i + 1] << 8) | (b[i + 2] << 16) | (b[i + 3] << 24)) >>> 0; }

  /// The zip's entries by name: {method, offset, size, compressed}.
  function zipEntries(b) {
    var end = -1;
    for (var i = b.length - 22; i >= 0 && i >= b.length - 22 - 65557; i--) {
      if (u32(b, i) === 0x06054b50) { end = i; break; }
    }
    if (end < 0) return null;
    var total = u16(b, end + 10);
    var at = u32(b, end + 16);
    var out = {};
    for (var k = 0; k < total; k++) {
      if (at + 46 > b.length || u32(b, at) !== 0x02014b50) return null;
      var method = u16(b, at + 10);
      var compressed = u32(b, at + 20);
      var size = u32(b, at + 24);
      var nameLen = u16(b, at + 28);
      var extraLen = u16(b, at + 30);
      var commentLen = u16(b, at + 32);
      var local = u32(b, at + 42);
      var name = new TextDecoder().decode(b.subarray(at + 46, at + 46 + nameLen));
      out[name] = { method: method, offset: local, size: size, compressed: compressed };
      at += 46 + nameLen + extraLen + commentLen;
    }
    return out;
  }

  async function inflate(bytes) {
    var DS = root.DecompressionStream;
    if (!DS) throw refuse("unsupported", "This browser cannot open .xlsx files. Save the sheet as .csv and choose that, or use a current Chrome, Edge, Safari or Firefox.");
    var stream = new root.Blob([bytes]).stream().pipeThrough(new DS("deflate-raw"));
    return new Uint8Array(await new root.Response(stream).arrayBuffer());
  }

  async function zipText(b, entries, name, budget) {
    var e = entries[name];
    if (!e) return null;
    if (e.size > budget.left) throw refuse("too-large", "");
    budget.left -= e.size;
    var at = e.offset;
    if (u32(b, at) !== 0x04034b50) return null;
    var start = at + 30 + u16(b, at + 26) + u16(b, at + 28);
    var data = b.subarray(start, start + e.compressed);
    var raw = e.method === 0 ? data : e.method === 8 ? await inflate(data) : null;
    if (!raw) return null;
    return new TextDecoder().decode(raw);
  }

  // ------------------------------------------------------------------ XML

  var ENTITY = { amp: "&", lt: "<", gt: ">", quot: '"', apos: "'" };
  function unescapeXml(s) {
    return String(s).replace(/&(#x[0-9a-f]+|#[0-9]+|[a-z]+);/gi, function (m, e) {
      if (e[0] === "#") {
        var cp = e[1] === "x" || e[1] === "X" ? parseInt(e.slice(2), 16) : parseInt(e.slice(1), 10);
        try { return String.fromCodePoint(cp); } catch (x) { return m; }
      }
      return ENTITY[e] !== undefined ? ENTITY[e] : m;
    });
  }
  function attr(tag, name) {
    var m = new RegExp("\\s" + name + "\\s*=\\s*(\"([^\"]*)\"|'([^']*)')").exec(tag);
    return m ? unescapeXml(m[2] !== undefined ? m[2] : m[3]) : null;
  }
  /// Every <t> inside `xml`, joined: a shared string with rich text runs is
  /// several of them. Phonetic hints (<rPh>) are not the text.
  function textsOf(xml) {
    var out = "";
    var clean = xml.replace(/<rPh\b[\s\S]*?<\/rPh>/g, "");
    var re = /<t(?:\s[^>]*)?>([\s\S]*?)<\/t>|<t(?:\s[^>]*)?\/>/g;
    var m;
    while ((m = re.exec(clean))) out += m[1] ? unescapeXml(m[1]) : "";
    return out;
  }

  // ----------------------------------------------------------------- XLSX

  var DATE_FORMATS = { 14: 1, 15: 1, 16: 1, 17: 1, 18: 2, 19: 2, 20: 2, 21: 2, 22: 3, 45: 2, 46: 2, 47: 2 };

  /// Which cell styles are dates: 1 date, 2 time, 3 date and time.
  function dateStyles(stylesXml) {
    var custom = {};
    if (!stylesXml) return [];
    var fm = /<numFmt\b[^>]*>/g;
    var m;
    while ((m = fm.exec(stylesXml))) {
      var id = Number(attr(m[0], "numFmtId"));
      var code = String(attr(m[0], "formatCode") || "").replace(/"[^"]*"|\[[^\]]*\]|\\./g, "");
      var day = /[dy]/i.test(code) || /m{3,}/i.test(code);
      var time = /[hs]/i.test(code);
      custom[id] = day && time ? 3 : day ? 1 : time ? 2 : 0;
    }
    var xfs = /<cellXfs\b[^>]*>([\s\S]*?)<\/cellXfs>/.exec(stylesXml);
    var out = [];
    if (!xfs) return out;
    var xr = /<xf\b[^>]*>/g;
    while ((m = xr.exec(xfs[1]))) {
      var f = Number(attr(m[0], "numFmtId") || 0);
      out.push(custom[f] !== undefined ? custom[f] : DATE_FORMATS[f] || 0);
    }
    return out;
  }

  function pad(n) { return (n < 10 ? "0" : "") + n; }
  /// A spreadsheet day number as ISO text: "2026-03-14", "09:30", or both.
  function serialToIso(serial, kind, from1904) {
    var v = Number(serial);
    if (!isFinite(v)) return String(serial);
    var ms = Math.round((v + (from1904 ? 1462 : 0) - 25569) * 86400000);
    var d = new Date(ms);
    if (isNaN(d.getTime())) return String(serial);
    var date = d.getUTCFullYear() + "-" + pad(d.getUTCMonth() + 1) + "-" + pad(d.getUTCDate());
    var time = pad(d.getUTCHours()) + ":" + pad(d.getUTCMinutes()) + (d.getUTCSeconds() ? ":" + pad(d.getUTCSeconds()) : "");
    if (kind === 2) return time;
    if (kind === 3 && (d.getUTCHours() || d.getUTCMinutes() || d.getUTCSeconds())) return date + " " + time;
    return date;
  }

  function columnIndex(ref) {
    var m = /^([A-Z]+)/.exec(String(ref || ""));
    if (!m) return -1;
    var n = 0;
    for (var i = 0; i < m[1].length; i++) n = n * 26 + (m[1].charCodeAt(i) - 64);
    return n - 1;
  }

  /// One worksheet's rows of text, stopping once it is past the row cap.
  function sheetRows(xml, shared, styles, from1904) {
    var rows = [];
    var body = /<sheetData\b[^>]*>([\s\S]*?)<\/sheetData>/.exec(xml);
    if (!body) return rows;
    var rowRe = /<row\b([^>]*)>([\s\S]*?)<\/row>|<row\b[^>]*\/>/g;
    var m;
    var next = 0;
    while ((m = rowRe.exec(body[1]))) {
      var r = m[1] !== undefined ? Number(attr("<row" + m[1] + ">", "r")) - 1 : next;
      if (!(r >= 0)) r = next;
      while (rows.length < r) rows.push([]);
      next = r + 1;
      var cells = [];
      var cellRe = /<c\b([^>]*?)(?:\/>|>([\s\S]*?)<\/c>)/g;
      var c;
      var col = 0;
      while ((c = cellRe.exec(m[2] || ""))) {
        var tag = "<c" + c[1] + ">";
        var at = columnIndex(attr(tag, "r"));
        if (at < 0) at = col;
        col = at + 1;
        var inner = c[2] || "";
        var type = attr(tag, "t") || "n";
        var v = /<v>([\s\S]*?)<\/v>/.exec(inner);
        var value = "";
        if (type === "s") value = v ? shared[Number(v[1])] || "" : "";
        else if (type === "inlineStr") value = textsOf(inner);
        else if (type === "b") value = v ? (v[1] === "1" ? "TRUE" : "FALSE") : "";
        else if (type === "str" || type === "e") value = v ? unescapeXml(v[1]) : "";
        else if (v) {
          var kind = styles[Number(attr(tag, "s") || 0)] || 0;
          value = kind ? serialToIso(v[1], kind, from1904) : v[1];
        }
        while (cells.length < at) cells.push("");
        cells[at] = value;
      }
      rows.push(cells);
      // Past the cap there is nothing to gain from reading on: the sheet
      // is refused, and says it is over the cap rather than how far.
      if (rows.length > MAX_ROWS + 1000) { rows.truncated = true; break; }
    }
    return rows;
  }

  /// An .xlsx as its sheets, each as rows of text, in the workbook's order.
  async function parseXlsx(bytes) {
    var b = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
    var entries = zipEntries(b);
    if (!entries || !entries["xl/workbook.xml"]) throw refuse("unreadable", "");
    var budget = { left: MAX_EXPANDED };
    var workbook = await zipText(b, entries, "xl/workbook.xml", budget);
    var rels = (await zipText(b, entries, "xl/_rels/workbook.xml.rels", budget)) || "";
    var sharedXml = (await zipText(b, entries, "xl/sharedStrings.xml", budget)) || "";
    var styles = dateStyles(await zipText(b, entries, "xl/styles.xml", budget));
    var from1904 = /<workbookPr\b[^>]*date1904\s*=\s*["'](1|true)["']/.test(workbook || "");
    var shared = [];
    var si = /<si\b[^>]*>([\s\S]*?)<\/si>|<si\b[^>]*\/>/g;
    var m;
    while ((m = si.exec(sharedXml))) shared.push(m[1] ? textsOf(m[1]) : "");
    var targets = {};
    var rr = /<Relationship\b[^>]*>/g;
    while ((m = rr.exec(rels))) targets[attr(m[0], "Id")] = attr(m[0], "Target");
    var sheets = [];
    var sr = /<sheet\b[^>]*>/g;
    var index = 0;
    while ((m = sr.exec(workbook || ""))) {
      index++;
      var id = attr(m[0], "r:id") || attr(m[0], "id");
      var target = String(targets[id] || "worksheets/sheet" + index + ".xml").replace(/^\//, "");
      var p = target.indexOf("xl/") === 0 ? target : "xl/" + target.replace(/^\.\//, "");
      var xml = await zipText(b, entries, p, budget);
      if (xml === null) continue;
      sheets.push({ name: attr(m[0], "name") || "Sheet " + index, rows: sheetRows(xml, shared, styles, from1904) });
    }
    return sheets;
  }

  // ---------------------------------------------------------------- read

  function extOf(name) {
    var m = /\.([a-z0-9]+)$/i.exec(String(name || ""));
    return m ? m[1].toLowerCase() : "";
  }

  /// The refusal for a file by its name and size alone, before a byte of
  /// it is read, or null.
  function fileProblem(name, size) {
    var file = String(name || "the file");
    var ext = extOf(file);
    if (ext === "xls" || ext === "numbers" || ext === "ods") {
      return refuse("wrong-kind", file + " is not a kind Krate reads here. Open it in Excel, Numbers or Google Sheets, save it as .xlsx or .csv, and choose that.");
    }
    if (ext !== "csv" && ext !== "tsv" && ext !== "txt" && ext !== "xlsx") {
      return refuse("wrong-kind", "Choose a .csv or .xlsx file.");
    }
    if (size > MAX_BYTES) {
      return refuse("too-large", file + " is " + mb(size) + ". Krate takes a spreadsheet up to 5 MB: keep only the sheet and the columns you need, save it, and choose it again.");
    }
    if (!size) return refuse("empty", file + " is empty. Choose a sheet with a header row and at least one row under it.");
    return null;
  }

  /// A chosen file, read: {file, sheets: [{name, rows}]}. Refused by kind
  /// and size before anything is parsed.
  async function read(name, bytes) {
    var file = String(name || "the file");
    var b = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes || []);
    var ext = extOf(file);
    var problem = fileProblem(file, b.length);
    if (problem) throw problem;
    if (ext === "xlsx") {
      var sheets;
      try {
        sheets = await parseXlsx(b);
      } catch (e) {
        if (e.reason === "unsupported") throw e;
        if (e.reason === "too-large") throw refuse("too-large", file + " opens up larger than Krate takes. Keep only the sheet and the columns you need, save it, and choose it again.");
        sheets = null;
      }
      if (!sheets || !sheets.length) {
        throw refuse("unreadable", file + " could not be read as a spreadsheet. Open it in Excel, Numbers or Google Sheets, save it again as .xlsx or .csv, and choose that.");
      }
      return { file: file, sheets: sheets };
    }
    var text = new TextDecoder("utf-8").decode(b);
    var stem = file.replace(/\.[^.]+$/, "");
    return { file: file, sheets: [{ name: stem, rows: parseCsv(text, ext === "tsv" ? "\t" : null) }] };
  }

  // --------------------------------------------------------------- table

  function blank(v) { return String(v === undefined || v === null ? "" : v).trim() === ""; }

  var BOOL = /^(yes|no|true|false|y|n)$/i;
  var NUMBER = /^[-+]?((\d{1,3}(,\d{3})+|\d+)(\.\d+)?|\.\d+)(e[-+]?\d+)?$/i;
  var MONEY = /^[-+]?\s*([$£€¥₹]|[A-Z]{3}\s)\s*(\d{1,3}(,\d{3})+|\d+)(\.\d+)?$|^[-+]?(\d{1,3}(,\d{3})+|\d+)(\.\d+)?\s*([$£€¥₹]|\s[A-Z]{3})$/;
  var PERCENT = /^[-+]?\d+(\.\d+)?\s*%$/;
  var DATE = /^(\d{4}-\d{1,2}-\d{1,2}([ T]\d{1,2}:\d{2}(:\d{2})?)?|\d{1,2}[/.]\d{1,2}[/.]\d{2,4}|\d{4}\/\d{1,2}\/\d{1,2})$/;
  var TIME = /^\d{1,2}:\d{2}(:\d{2})?(\s?[ap]m)?$/i;
  var EMAIL = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;

  /// What kind of value a column holds, from every value in it: "number",
  /// "money", "percent", "date", "time", "yes/no", "email", "text", or
  /// "empty" when nothing is in it.
  function inferType(values) {
    var seen = values.map(function (v) { return String(v).trim(); }).filter(function (v) { return v !== ""; });
    if (!seen.length) return "empty";
    function all(re) { return seen.every(function (v) { return re.test(v); }); }
    if (all(BOOL)) return "yes/no";
    if (all(NUMBER)) return "number";
    if (seen.every(function (v) { return MONEY.test(v) || NUMBER.test(v); }) && seen.some(function (v) { return MONEY.test(v); })) return "money";
    if (all(PERCENT)) return "percent";
    if (all(DATE)) return "date";
    if (all(TIME)) return "time";
    if (all(EMAIL)) return "email";
    return "text";
  }

  /// The header row and the rows under it: empty rows dropped, every row
  /// as wide as the widest, a column with no name and no values dropped,
  /// names made unique ("Amount", "Amount 2").
  function table(rows) {
    var kept = (rows || []).filter(function (r) { return r && r.some(function (v) { return !blank(v); }); });
    if (!kept.length) return { columns: [], rows: [], truncated: Boolean(rows && rows.truncated) };
    var width = 0;
    kept.forEach(function (r) { width = Math.max(width, r.length); });
    var head = kept[0];
    var body = kept.slice(1).map(function (r) {
      var out = [];
      for (var i = 0; i < width; i++) out.push(r[i] === undefined || r[i] === null ? "" : String(r[i]).trim());
      return out;
    });
    var keep = [];
    for (var c = 0; c < width; c++) {
      var named = !blank(head[c]);
      if (named || body.some(function (r) { return r[c] !== ""; })) keep.push(c);
    }
    var used = {};
    var columns = keep.map(function (c, k) {
      var base = blank(head[c]) ? "Column " + (k + 1) : String(head[c]).replace(/\s+/g, " ").trim();
      var name = base;
      for (var n = 2; used[name.toLowerCase()]; n++) name = base + " " + n;
      used[name.toLowerCase()] = true;
      return { name: name, type: inferType(body.map(function (r) { return r[c]; })) };
    });
    return { columns: columns, rows: body.map(function (r) { return keep.map(function (c) { return r[c]; }); }), truncated: Boolean(rows.truncated) };
  }

  /// The refusal for a table Krate will not start from, or null.
  function check(t, file) {
    var name = String(file || "The sheet");
    if (!t || !t.columns.length || !t.rows.length) {
      return refuse("empty", name + " has no rows to read. Choose a sheet with a header row and at least one row under it.");
    }
    if (t.rows.length > MAX_ROWS || t.truncated) {
      return refuse("too-many-rows", name + " has " + (t.truncated ? "more than " + count(MAX_ROWS) : count(t.rows.length)) + " rows. Krate takes up to " + count(MAX_ROWS) + ": keep the rows you need, save it, and choose it again.");
    }
    return null;
  }

  function csvCell(v) {
    var s = String(v === undefined || v === null ? "" : v);
    return /[",\r\n]|^\s|\s$/.test(s) ? '"' + s.replace(/"/g, '""') + '"' : s;
  }
  /// The whole table as CSV: the header, then every row.
  function toCsv(t) {
    var lines = [t.columns.map(function (c) { return csvCell(c.name); }).join(",")];
    t.rows.forEach(function (r) { lines.push(r.map(csvCell).join(",")); });
    return lines.join("\n") + "\n";
  }

  // -------------------------------------------------------------- request

  function clip(s, n) {
    var a = Array.from(String(s || "").replace(/\s+/g, " ").trim());
    return a.length > n ? a.slice(0, n - 1).join("") + "\u2026" : a.join("");
  }
  /// The asset's file name: the sheet's own name, as letters, digits and
  /// dashes ("Q3 stock (final).xlsx" -> "q3-stock-final.csv").
  function assetName(file) {
    var stem = String(file || "").replace(/\.[^.]+$/, "").toLowerCase().normalize("NFKD").replace(/[\u0300-\u036f]/g, "")
      .replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 40);
    return (stem || "records") + ".csv";
  }

  /// The request a Studio build starts from, at most 2,000 characters: what
  /// the app is for, the columns with their kinds, as many of the first
  /// rows as fit (up to five), and how the app keeps and imports the
  /// records. The whole sheet goes beside it as `asset`, never inside it.
  function request(o) {
    var t = o.table;
    var file = String(o.file || "the spreadsheet");
    var asset = assetName(file);
    var app = clip(o.app || "", 60) || "My records";
    var purpose = clip(o.purpose || "", 300);
    var org = o.org && o.org.slug ? clip(o.org.name || o.org.slug, 60) : "";
    var n = t.rows.length;
    var head = app + ": " + (purpose || "an app for the records in " + file) + "\n\n";
    var keeps = org
      ? "Keep the records as " + org + "'s team data with store.org, one record per row, so everyone in " + org + " sees the same records."
      : "Keep the records on this computer with store.sql, one row per record.";
    var importing = org
      ? " On the first run, once a sync has reached " + org + " (org::who() answers) and there are no records yet, read it with krate::assets::read_text(\"" + asset + "\") and add every row as a record, with its row number as the id, so two people importing at once write the same records."
      : " On the first run, when there are no records yet, read it with krate::assets::read_text(\"" + asset + "\") and add every row as a record.";
    var tail = "\n\nThe whole sheet is attached as attached/" + asset + ". Copy it unchanged into the app's assets/ folder and do not read all of it: the columns and rows above are what it holds. " + keeps +
      importing + " Then people add, change, search and remove records.";
    function columnsLine(limit) {
      var shown = t.columns.slice(0, limit).map(function (c) { return clip(c.name, 40) + " (" + c.type + ")"; });
      var more = t.columns.length - shown.length;
      return "It starts from the spreadsheet " + clip(file, 80) + ": " + count(n) + (n === 1 ? " row" : " rows") + " and " + t.columns.length +
        (t.columns.length === 1 ? " column" : " columns") + ": " + shown.join(", ") + (more > 0 ? ", and " + more + " more" : "") + ".";
    }
    var limit = t.columns.length;
    var cols = columnsLine(limit);
    while (limit > 3 && (head + cols + tail).length > MAX_REQUEST - 200) cols = columnsLine(--limit);
    var line = function (cells) { return cells.slice(0, limit).map(function (v) { return clip(v, CELL_CLIP); }).join(" | "); };
    var text = head + cols;
    var sample = [];
    var top = "\n\nThe first rows:\n" + line(t.columns.map(function (c) { return c.name; }));
    for (var i = 0; i < Math.min(SAMPLE_ROWS, n); i++) {
      var next = sample.concat([line(t.rows[i])]);
      if ((text + top + "\n" + next.join("\n") + tail).length > MAX_REQUEST) break;
      sample = next;
    }
    if (sample.length) text += top + "\n" + sample.join("\n");
    text += tail;
    if (text.length > MAX_REQUEST) text = Array.from(text).slice(0, MAX_REQUEST).join("");
    return { request: text, asset: { name: asset, text: toCsv(t) }, sampleRows: sample.length };
  }

  root.KrateSheet = {
    MAX_BYTES: MAX_BYTES,
    MAX_ROWS: MAX_ROWS,
    read: read,
    fileProblem: fileProblem,
    parseCsv: parseCsv,
    parseXlsx: parseXlsx,
    table: table,
    inferType: inferType,
    check: check,
    toCsv: toCsv,
    assetName: assetName,
    request: request,
  };
})(typeof window !== "undefined" ? window : globalThis);
