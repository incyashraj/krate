"""Make the spreadsheet fixtures krate::sheets is tested against (IC-993).

    python3 crates/bindings-rust/tests/fixtures/sheets/make.py

openpyxl.xlsx is written by openpyxl (the library behind most Python
exports). office.ods is a hand-made OpenDocument file in the shape
LibreOffice saves: padding rows repeated a million times, a cell comment,
a merged cell with text left under it,
spaces as <text:s>, a formula's cached value, a percentage, a currency, a
time as a duration. Both carry the same rows, so one expectation in the
Rust test covers each format. Rerunning gives byte-identical files.
"""

import datetime as dt
import os
import zipfile

HERE = os.path.dirname(os.path.abspath(__file__))


def make_xlsx():
    import openpyxl

    wb = openpyxl.Workbook()
    ws = wb.active
    ws.title = "Orders"
    ws.append(["Item", "Qty", "Price", "Ordered", "Paid"])
    ws.append(["Paper A4", 10, 4.95, dt.date(2024, 3, 15), True])
    ws.append(["Zürich café ☕ 東京", -3, 0.1, dt.datetime(2024, 12, 31, 23, 59, 58), False])
    ws.append([" spaced ", 1e21, 1.5e-7, dt.time(9, 30), None])
    ws["A6"] = "after a gap"
    ws["E6"] = "far right"
    ws["F2"] = "=B2*C2"  # openpyxl stores no cached value: reads as empty
    ws2 = wb.create_sheet("Ünïcödé & <more>")
    ws2["B2"] = "only B2"
    wb.create_sheet("Empty")
    for part in ("openpyxl.xlsx",):
        path = os.path.join(HERE, part)
        wb.save(path)
        normalise(path)


def normalise(path):
    """Fix the timestamps openpyxl writes, so the file is reproducible."""
    with zipfile.ZipFile(path) as z:
        items = [(i.filename, z.read(i.filename)) for i in z.infolist()]
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as z:
        for name, data in items:
            if name == "docProps/core.xml":
                import re

                data = re.sub(rb"\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ", b"2024-01-01T00:00:00Z", data)
            info = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
            info.compress_type = zipfile.ZIP_DEFLATED
            z.writestr(info, data)


CONTENT = """<?xml version="1.0" encoding="UTF-8"?>
<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:of="urn:oasis:names:tc:opendocument:xmlns:of:1.2" xmlns:calcext="urn:org:documentfoundation:names:experimental:calc:xmlns:calcext:1.0" office:version="1.3"><office:body><office:spreadsheet>
<table:table table:name="Orders" table:style-name="ta1"><table:table-column table:style-name="co1" table:number-columns-repeated="6" table:default-cell-style-name="Default"/>
<table:table-row table:style-name="ro1"><table:table-cell office:value-type="string" calcext:value-type="string"><text:p>Item</text:p></table:table-cell><table:table-cell office:value-type="string" calcext:value-type="string"><text:p>Qty</text:p></table:table-cell><table:table-cell office:value-type="string" calcext:value-type="string"><text:p>Price</text:p></table:table-cell><table:table-cell office:value-type="string" calcext:value-type="string"><text:p>Ordered</text:p></table:table-cell><table:table-cell office:value-type="string" calcext:value-type="string"><text:p>Paid</text:p></table:table-cell><table:table-cell table:number-columns-repeated="1019"/></table:table-row>
<table:table-row table:style-name="ro1"><table:table-cell office:value-type="string" calcext:value-type="string"><text:p>Paper A4</text:p><office:annotation><dc:creator>Someone</dc:creator><dc:date>2024-03-15T10:00:00</dc:date><text:p>a comment, not the value</text:p></office:annotation></table:table-cell><table:table-cell office:value-type="float" office:value="10" calcext:value-type="float"><text:p>10</text:p></table:table-cell><table:table-cell office:value-type="currency" office:currency="CHF" office:value="4.95" calcext:value-type="currency"><text:p>CHF 4.95</text:p></table:table-cell><table:table-cell table:style-name="ce1" office:value-type="date" office:date-value="2024-03-15" calcext:value-type="date"><text:p>15.03.24</text:p></table:table-cell><table:table-cell office:value-type="boolean" office:boolean-value="true" calcext:value-type="boolean"><text:p>TRUE</text:p></table:table-cell><table:table-cell table:formula="of:=[.B2]*[.C2]" office:value-type="float" office:value="49.5" calcext:value-type="float"><text:p>49.5</text:p></table:table-cell><table:table-cell table:number-columns-repeated="1018"/></table:table-row>
<table:table-row table:style-name="ro1"><table:table-cell office:value-type="string" calcext:value-type="string"><text:p>Z<text:span text:style-name="T1">ü</text:span>rich café ☕ 東京</text:p></table:table-cell><table:table-cell office:value-type="float" office:value="-3" calcext:value-type="float"><text:p>-3</text:p></table:table-cell><table:table-cell office:value-type="percentage" office:value="0.1" calcext:value-type="percentage"><text:p>10%</text:p></table:table-cell><table:table-cell office:value-type="date" office:date-value="2024-12-31T23:59:58" calcext:value-type="date"><text:p>31.12.24 23:59:58</text:p></table:table-cell><table:table-cell office:value-type="boolean" office:boolean-value="false" calcext:value-type="boolean"><text:p>FALSE</text:p></table:table-cell><table:table-cell table:formula="of:=1/0" office:value-type="string" office:string-value="" calcext:value-type="error"><text:p>#DIV/0!</text:p></table:table-cell><table:table-cell table:number-columns-repeated="1018"/></table:table-row>
<table:table-row table:style-name="ro1"><table:table-cell office:value-type="string" calcext:value-type="string"><text:p><text:s/>spaced<text:s/></text:p></table:table-cell><table:table-cell office:value-type="float" office:value="1E+21" calcext:value-type="float"><text:p>1E+21</text:p></table:table-cell><table:table-cell office:value-type="float" office:value="1.5E-07" calcext:value-type="float"><text:p>0</text:p></table:table-cell><table:table-cell office:value-type="time" office:time-value="PT09H30M00S" calcext:value-type="time"><text:p>09:30:00</text:p></table:table-cell><table:table-cell table:number-columns-repeated="1020"/></table:table-row>
<table:table-row table:style-name="ro1"><table:table-cell table:number-columns-repeated="1024"/></table:table-row>
<table:table-row table:style-name="ro1"><table:table-cell office:value-type="string" calcext:value-type="string"><text:p>after a gap</text:p></table:table-cell><table:table-cell table:number-columns-repeated="3"/><table:table-cell office:value-type="string" calcext:value-type="string"><text:p>far right</text:p></table:table-cell><table:table-cell table:number-columns-repeated="1019"/></table:table-row>
<table:table-row table:style-name="ro1" table:number-rows-repeated="1048570"><table:table-cell table:number-columns-repeated="1024"/></table:table-row>
</table:table>
<table:table table:name="Ünïcödé &amp; &lt;more&gt;" table:style-name="ta1"><table:table-column table:style-name="co1" table:number-columns-repeated="2" table:default-cell-style-name="Default"/><table:table-row table:style-name="ro1"><table:table-cell table:number-columns-repeated="2"/></table:table-row><table:table-row table:style-name="ro1"><table:table-cell/><table:table-cell table:number-columns-spanned="2" table:number-rows-spanned="1" office:value-type="string" calcext:value-type="string"><text:p>only B2</text:p></table:table-cell><table:covered-table-cell office:value-type="string" calcext:value-type="string"><text:p>hidden under the merge</text:p></table:covered-table-cell></table:table-row></table:table>
<table:table table:name="Empty" table:style-name="ta1"><table:table-column table:style-name="co1" table:default-cell-style-name="Default"/><table:table-row table:style-name="ro1"><table:table-cell/></table:table-row></table:table>
<table:named-expressions/></office:spreadsheet></office:body></office:document-content>
"""

MANIFEST = """<?xml version="1.0" encoding="UTF-8"?>
<manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0" manifest:version="1.3">
 <manifest:file-entry manifest:full-path="/" manifest:version="1.3" manifest:media-type="application/vnd.oasis.opendocument.spreadsheet"/>
 <manifest:file-entry manifest:full-path="content.xml" manifest:media-type="text/xml"/>
</manifest:manifest>
"""


def make_ods():
    path = os.path.join(HERE, "office.ods")
    with zipfile.ZipFile(path, "w") as z:
        stamp = (1980, 1, 1, 0, 0, 0)
        info = zipfile.ZipInfo("mimetype", date_time=stamp)
        info.compress_type = zipfile.ZIP_STORED
        z.writestr(info, "application/vnd.oasis.opendocument.spreadsheet")
        for name, data in (("content.xml", CONTENT), ("META-INF/manifest.xml", MANIFEST)):
            info = zipfile.ZipInfo(name, date_time=stamp)
            info.compress_type = zipfile.ZIP_DEFLATED
            z.writestr(info, data.encode("utf-8"))


if __name__ == "__main__":
    make_xlsx()
    make_ods()
