#!/usr/bin/env python3
"""Generate the answer pages from one template.

Each page answers one question somebody actually types into a search box, and
answers it completely, because a page that ranks for a question and then does
not answer it is worse than not ranking.

They share a template rather than being written by hand four times: the previous
round of this site ended up with four copies of the same stylesheet, two of them
already drifted, and prose pages drift the same way.

    python3 scripts/build-answer-pages.py
"""

import html
import json
import re
import sys
import importlib.util
from datetime import date
import unittest
from html.parser import HTMLParser
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LANDING = ROOT / "docs" / "landing" / "index.html"
OUT = ROOT / "docs" / "answers"
# Committed beside llms.txt, so the deploy's copy of docs/landing serves it.
LLMS_FULL = ROOT / "docs" / "landing" / "llms-full.txt"
ANSWERS_HEADING = "# Answers about shipping desktop apps with Krate"
# The release the reviewed pages were checked against.
REVIEWED_VERSION = "0.5.4"
sys.path.insert(0, str(Path(__file__).resolve().parent))
import site_kit as K  # noqa: E402  the header, phone menu and footer every kit page shares

ARROW = K.ARROW

# Page-only styles, on the kit's tokens. The page data below keeps its own
# class names (answer-cmd, answer-table, shipping-lane): the content is the
# same content the pages had before the 2026-10 redesign, restyled here.
#
# Command blocks must stay unwrapped -- a shell line broken across lines is a
# line somebody pastes wrong -- so a block scrolls inside its own box and
# never widens the page (at 390px one `krate run ... --grant` line once made
# every answer page scroll sideways).
ANSWER_CSS = """<style>
@media (min-width: 961px) { .ph.art { padding-left: 276px; } }
.ph.art .lede { max-width: 640px; }
.ph.art .acts { margin-top: 26px; gap: 10px 12px; }
.ph.art .ent { margin-top: 18px; max-width: 640px; font-size: 14.5px; line-height: 1.6; color: var(--mute); }
.ph.art .ent a, .ph.art .lede a { color: var(--accent); }
.ph.art .ent code, .ph.art .lede code { font: 500 .88em ui-monospace, "SF Mono", Menlo, monospace; padding: 2px 6px; border-radius: 6px; background: var(--s2); color: var(--ink); }
.art-body { padding-top: 64px; }
.prose { max-width: 700px; min-width: 0; }
.prose > :first-child { margin-top: 0; }
.prose section + section { margin-top: 52px; }
.prose section > h2 { margin-top: 0; }
.prose :is(p, li, h2, h3) { overflow-wrap: anywhere; }
.answer-table :is(td, th) { overflow-wrap: normal; hyphens: manual; }
.answer-table tbody th { min-width: 150px; }
.prose em { font-style: italic; }
.ph.art h1 .nw { white-space: nowrap; }
.prose section[id] { scroll-margin-top: calc(var(--hd) + 20px); }
:root[data-theme="dark"] .prose pre { box-shadow: inset 0 0 0 1px var(--line-2); }
.prose pre.answer-cmd { max-width: 100%; overflow-x: auto; white-space: pre-wrap; overflow-wrap: anywhere; overscroll-behavior-x: contain; -webkit-overflow-scrolling: touch; }
/* the comparison table */
.answer-table-wrap { margin-top: 22px; border-radius: 18px; background: var(--s1); box-shadow: var(--sh-1); overflow-x: auto; }
.answer-table { width: 100%; min-width: 640px; border-collapse: collapse; font-size: 14.5px; }
.answer-table caption { caption-side: top; text-align: left; padding: 14px 14px 6px; font-size: 12.5px; color: var(--ink-3); }
.answer-table thead th { text-align: left; font-weight: 500; font-size: 12.5px; color: var(--ink-3); padding: 10px 14px; border-bottom: 1px solid var(--line); }
.answer-table tbody th { text-align: left; vertical-align: top; padding: 13px 14px; border-bottom: 1px solid var(--line); font-weight: 500; color: var(--ink); line-height: 1.5; }
.answer-table td { padding: 13px 14px; border-bottom: 1px solid var(--line); color: var(--ink-2); vertical-align: top; line-height: 1.5; }
.answer-table tbody tr:last-child > * { border-bottom: 0; }
/* the two shipping workflows, side by side */
.prose .shipping-flow { margin-top: 26px; display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 14px; }
.prose .shipping-flow figcaption { grid-column: 1 / -1; font-size: 13.5px; line-height: 1.55; color: var(--ink-3); }
.prose .shipping-lane { padding: 20px 20px 8px; border-radius: 18px; background: var(--soft); }
.prose .shipping-lane-krate { background: var(--accent-soft); }
.prose .shipping-lane h3 { margin: 0 0 10px; font-size: 16px; }
.prose .shipping-lane-krate h3 { color: var(--accent); }
.prose .shipping-lane ol { list-style: none; padding: 0; }
.prose .shipping-lane li { margin: 0; padding: 11px 0 12px; border-top: 1px solid var(--line-2); font-size: 14.5px; line-height: 1.5; color: var(--ink); }
.prose .shipping-lane-krate li { border-top-color: color-mix(in srgb, var(--accent) 18%, transparent); }
.prose .shipping-lane li span { display: block; margin-bottom: 2px; font-size: 12px; font-weight: 500; color: var(--ink-3); }
/* the closing band: words, then the ways forward */
.band.st { flex-direction: column; align-items: flex-start; gap: 22px; }
.band.st p { max-width: 640px; }
.band.st .acts { margin-top: 0; justify-content: flex-start; gap: 10px 12px; }
.band p a { color: var(--accent); }
@media (max-width: 680px) {
  .art-body { padding-top: 44px; }
  .prose .shipping-flow { grid-template-columns: 1fr; }
  .prose pre.answer-cmd { font-size: 12.5px; }
  .ph.art .acts, .band.st .acts { flex-direction: column; align-items: stretch; width: 100%; }
  .band .acts .btn { white-space: normal; text-align: center; }
}
</style>"""


PUBLISHER = {"@type": "Organization", "@id": "https://krate.tech/#organization",
             "name": "Krate Labs", "url": "https://krate.tech/"}
RUNTIME = {"@type": "SoftwareApplication", "@id": "https://krate.tech/#runtime", "name": "Krate",
           "applicationCategory": "DeveloperApplication", "operatingSystem": "macOS, Windows, Linux"}


class PlainText(HTMLParser):
    """Readable text from a page fragment, for structured data and llms-full.txt."""

    BLOCKS = {"p", "pre", "h3", "caption", "ol", "ul", "div", "table"}

    def __init__(self, links=False):
        super().__init__(convert_charrefs=True)
        self.out = []
        self.links = links
        self.href = []

    def handle_starttag(self, tag, attrs):
        if tag in self.BLOCKS or tag in ("li", "tr"):
            self.out.append("\n")
        if tag == "li":
            self.out.append("- ")
        if tag in ("td", "th"):
            self.out.append(" | ")
        if tag == "a":
            href = dict(attrs).get("href", "")
            self.href.append("https://krate.tech" + href if href.startswith("/") else href)

    def handle_endtag(self, tag):
        if tag in self.BLOCKS:
            self.out.append("\n")
        if tag == "a" and self.href:
            href = self.href.pop()
            if self.links and href.startswith("http"):
                self.out.append(f" ({href})")

    def handle_data(self, data):
        self.out.append(data)


def plain_text(fragment, links=False):
    parser = PlainText(links)
    parser.feed(fragment)
    parser.close()
    lines = (re.sub(r"[ \t\r\f\v]+", " ", line).strip() for line in "".join(parser.out).split("\n"))
    text = "\n".join(line.lstrip("| ").strip() if line.startswith("|") else line for line in lines)
    return re.sub(r"\n{3,}", "\n\n", text).strip()


def page_schema(page, url):
    schema = {
        "@context": "https://schema.org", "@type": "FAQPage" if page.get("faq") else "WebPage",
        "@id": url + "#webpage", "url": url, "name": page["title"],
        "description": page["description"], "inLanguage": "en",
        "isPartOf": {"@type": "WebSite", "@id": "https://krate.tech/#website", "name": "Krate", "url": "https://krate.tech/"},
        "publisher": PUBLISHER, "about": RUNTIME,
    }
    if page.get("reviewed"):
        schema["dateModified"] = page["reviewed"]
    if page.get("faq"):
        schema["mainEntity"] = [
            {"@type": "Question", "name": question,
             "acceptedAnswer": {"@type": "Answer", "text": plain_text(answer)}}
            for question, answer in page["faq"]
        ]
    return schema


def reviewed_line(page):
    if not page.get("reviewed"):
        return ""
    day = date.fromisoformat(page["reviewed"])
    # Inline, at the start of the page's meta line: "Reviewed 6 October 2026
    # against Krate 0.5.4. 6 min read".
    return (f'Reviewed <time datetime="{day.isoformat()}">'
            f'{day.day} {day:%B %Y}</time> against Krate {REVIEWED_VERSION}. ')


def faq_section(page):
    if not page.get("faq"):
        return ""
    items = "\n".join(f"""      <h3>{html.escape(question)}</h3>
{answer}""" for question, answer in page["faq"])
    return f"""    <section id="questions">
      <h2>Questions</h2>
{items}
    </section>
"""


def related_section(page):
    links = "\n".join(
        f'        <li><a href="/{other["slug"]}">{html.escape(other["h1"])}</a></li>'
        for other in PAGES if other["slug"] != page["slug"])
    return f"""    <section id="more-answers">
      <h2>More answers about making and sending apps</h2>
      <ul class="answer-related">
{links}
      </ul>
    </section>
"""


def page_head(page):
    """The head of one answer page: its own title, description, canonical,
    social cards and structured data, then the kit."""
    title = html.escape(page["title"], quote=True)
    description = html.escape(page["description"], quote=True)
    url = "https://krate.tech/" + page["slug"]
    schema = json.dumps(page_schema(page, url), ensure_ascii=False).replace("<", "\\u003c")
    return f'''<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover">
<title>{title}</title>
<meta name="description" content="{description}">
<link rel="canonical" href="{url}">
<meta property="og:type" content="website">
<meta property="og:site_name" content="Krate">
<meta property="og:title" content="{title}">
<meta property="og:description" content="{description}">
<meta property="og:url" content="{url}">
<meta property="og:image" content="https://krate.tech/og-v5.png">
<meta property="og:image:alt" content="Krate: make and send apps like pictures">
<meta name="twitter:card" content="summary_large_image">
<meta name="twitter:title" content="{title}">
<meta name="twitter:description" content="{description}">
<meta name="twitter:image" content="https://krate.tech/og-v5.png">
<link rel="icon" href="/krate-favicon.png">
<meta name="theme-color" content="#fbfbfd" media="(prefers-color-scheme: light)">
<meta name="theme-color" content="#08080a" media="(prefers-color-scheme: dark)">
<script type="application/ld+json">{schema}</script>
{K.HEAD_THEME}
{K.KIT_LINKS}
{ANSWER_CSS}
</head>'''


def heading_html(text):
    # A hyphenated word in a heading can break at its hyphen ("AI-" at the end
    # of one line, "built" on the next); keep those words whole.
    return re.sub(r"\b(\w+-\w+)\b", r'<span class="nw">\1</span>', html.escape(text))


def section_id(title):
    return re.sub(r"[^a-z0-9]+", "-", title.lower()).strip("-")


def read_minutes(page):
    words = len(re.sub(r"<[^>]+>", " ", page["lead"] + " ".join(b for _, b in page["sections"])).split())
    return max(1, round(words / 220))


def render(page):
    head = page_head(page)

    section_ids = [section_id(title) for title, _ in page["sections"]] + ["more-answers"]
    if page.get("faq"):
        section_ids.append("questions")
    if len(section_ids) != len(set(section_ids)) or not all(section_ids):
        raise ValueError(f"Section anchors must be unique: {page['slug']}")
    actions = "\n".join(
        (f'    <a class="btn dark" href="{html.escape(url, quote=True)}">{html.escape(label)} {ARROW}</a>' if index == 0
         else f'    <a class="btn ghost" href="{html.escape(url, quote=True)}">{html.escape(label)}</a>')
        for index, (label, url) in enumerate(page["entry_actions"])
    )
    entry_note = f'  <p class="ent">{page["entry_note"]}</p>\n' if page.get("entry_note") else ""
    toc = "\n".join(f'      <a href="#{section_id(t)}">{html.escape(t)}</a>' for t, _ in page["sections"])
    if page.get("faq"):
        toc += '\n      <a href="#questions">Questions</a>'
    toc += '\n      <a href="#more-answers">More answers</a>'
    sections = "\n".join(
        f'''      <section id="{section_id(t)}">
        <h2>{html.escape(t)}</h2>
{b}
      </section>'''
        for t, b in page["sections"]
    )
    sections = sections.replace('<pre class="answer-cmd">',
                                '<pre class="answer-cmd" tabindex="0" aria-label="Command example">')

    return f"""{head}
<body>
{K.HEADER}
{K.MNAV}

<main id="main">
<section class="ph sm l art wrap">
  <span class="eye">Answer</span>
  <h1>{heading_html(page["h1"])}</h1>
  <p class="lede">{page["lead"]}</p>
  <p class="meta">{reviewed_line(page)}{read_minutes(page)} min read</p>
  <nav class="acts answer-actions" aria-label="Choose your next step">
{actions}
  </nav>
{entry_note}</section>

<section class="art-body wrap">
  <div class="doc">
    <nav class="toc" aria-label="On this page">
      <h6>On this page</h6>
{toc}
    </nav>
    <article class="prose">
{sections}
{faq_section(page)}{related_section(page)}    </article>
  </div>
</section>

<div class="band st rv">
  <div><h2>Make your first app</h2><p>Describe it and Krate makes it. Send it as a link that opens in any browser, phones included, and run the same file natively on Mac, Windows and Linux. The runtime, format and CLI are open source under MIT OR Apache-2.0 (<a href="https://github.com/incyashraj/krate#license">licensing details</a>), and the <a href="/docs/limits.html">capability reference</a> lists everything an app can use today.</p></div>
  <div class="acts">
    <a class="btn dark" href="/studio/">Make an app in Studio {ARROW}</a>
    <a class="btn ghost" href="/docs/quickstart.html">Developer quickstart</a>
    <a class="btn ghost" href="/docs/porting.html">Bring your app over</a>
    <a class="btn link" href="https://github.com/incyashraj/krate">Explore Krate on GitHub</a>
  </div>
</div>
</main>

{K.FOOTER}
{K.KIT_SCRIPT}
</body>
</html>
"""


PIN = "59aeeebc8dafa5ec1700e01a44f8fe8fe7ab980e"
E3 = "https://github.com/incyashraj/krate/blob/" + PIN + "/evidence/e3/run-36557127592-replay-"
CHART = "https://raw.githubusercontent.com/incyashraj/krate/c46d29500f2b894c89285b04e1b5e2f75184e972/evidence/ported/chart.krate"
EL_SIGN = "https://www.electronjs.org/docs/latest/tutorial/code-signing"
EL_SANDBOX = "https://www.electronjs.org/docs/latest/tutorial/sandbox"
EL_DIST = "https://www.electronjs.org/docs/latest/tutorial/distribution-overview"
EL_PROC = "https://www.electronjs.org/docs/latest/tutorial/process-model"
TA_DIST = "https://tauri.app/distribute/"
TA_CAPS = "https://tauri.app/security/capabilities/"
TA_WEBVIEW = "https://tauri.app/reference/webview-versions/"
MS_SS = "https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/smartscreen-reputation"
MDN_FSA = "https://developer.mozilla.org/en-US/docs/Web/API/Window/showOpenFilePicker"

# The date this copy was last checked against the release named in
# REVIEWED_VERSION. Competitor facts keep the date they were read, in each
# table's caption.
REVIEWED = "2026-10-27"

THREE_OS_REPLAY = f"""<p>Ten ported apps, as the same committed <code>.krate</code> files, passed their headless replay checks on macOS (Apple silicon), Ubuntu (x86_64) and Windows (x86_64) GitHub runners with runtime 0.5.4 on 29 September 2026. Records: <a href="{E3}macos.tsv">macOS</a>, <a href="{E3}ubuntu.tsv">Ubuntu</a>, <a href="{E3}windows.tsv">Windows</a>. Native runtimes are published for Apple silicon and Intel Macs, x64 and ARM64 Windows, and x86_64 and arm64 Linux.</p>"""

# The measured comparison, in the exact figures evidence/claims/performance.json
# vouches for (scripts/check-claims.py holds every page to them), always with
# its date, machine and workload.
MEASURED = """<p>Krate's Notes app is one 36.6 KiB file. Holding 50,000 lines, it used 178.5 MiB of memory against 2,299.4 MiB for MarkText, an Electron editor: 13x less. A warm open took 237.1 ms against 611.5 ms: 2.6x faster. MarkText takes 284.6 MiB installed; the Krate runtime is installed once and shared by every app.</p>
<p>Measured on 25 August 2026 on an Apple M4: Krate's Notes app against MarkText 0.17.1 on the same 50,000-line notes workload, an equivalent workload. <a href="/reports/">Read the method</a>.</p>"""

# How a link opens, said the same way everywhere.
LINK_OPENS = "it opens in any modern browser, phones included, with nothing to install and no account"


def comparison_table(caption, label, head, rows):
    h = "".join(f'<th scope="col">{c}</th>' for c in head)
    body = "\n".join("<tr>" + f'<th scope="row">{r[0]}</th>' + "".join(f"<td>{c}</td>" for c in r[1:]) + "</tr>" for r in rows)
    return (f'<div class="answer-table-wrap" role="region" aria-label="{label}" tabindex="0"><table class="answer-table">\n'
            f"<caption>{caption}</caption>\n<thead><tr>{h}</tr></thead>\n<tbody>\n{body}\n</tbody></table></div>")


PAGES = [
    {
        "slug": "portable-desktop-app-format.html",
        "title": "The .krate format: one app file for every screen | Krate",
        "description": "One .krate file runs natively on macOS, Windows and Linux and opens from a link in any browser, phones included, with nothing to install.",
        "h1": "The .krate format: one app file for every screen",
        "lead": f"A <code>.krate</code> is one small app file for every screen: the same bytes run natively on macOS, Windows and Linux, and sent as a link, {LINK_OPENS}. The app starts with no access to your files or the network and asks before it touches either.",
        "reviewed": REVIEWED,
        "faq": [
            ("What does the person opening a .krate need?", "<p>Nothing, to open a link: the app runs in any modern browser, phones included, with no account. To run it natively, the Krate runtime, installed once. They never need Rust, your source or your build tools.</p>"),
            ("Can a .krate file run on a phone?", "<p>Yes. Send it as a link and it opens in the phone's browser, with nothing to install and no account.</p>"),
            ("Is a .krate the same as a Rust crate?", "<p>No. A crate is a Rust package on crates.io. A <code>.krate</code> file is a packaged application: a WebAssembly component, its manifest, and optional source and assets.</p>"),
            ("How do I check that two copies are the same file?", "<p>Compare SHA-256 hashes: <code>shasum -a 256</code> on macOS, <code>sha256sum</code> on Linux, <code>Get-FileHash</code> in Windows PowerShell. Matching hashes mean identical bytes.</p>"),
        ],
        "entry_actions": [("Try the same-file workflow", "/docs/quickstart.html"),
                          ("Bring your project over", "/docs/porting.html")],
        "sections": [
            ("One file, every screen", f"""<p>You build the app once and get one <code>.krate</code> file. Send it as a link and {LINK_OPENS}. On Mac, Windows and Linux, the Krate runtime runs the same file natively, as a real window with no browser or WebView inside. The runtime is installed once and every Krate app after that is just a file.</p>
<p>There is no build per operating system, no build per processor and no package per platform to sign. Krate does the per-platform work in its runtimes, once, for every app. See <a href="/desktop-app-distribution.html">how that compares with Electron and Tauri</a>.</p>"""),
            ("Try one file tested on all three systems", """<p>Download <a href="https://raw.githubusercontent.com/incyashraj/krate/c46d29500f2b894c89285b04e1b5e2f75184e972/evidence/ported/chart.krate">chart.krate</a>, a bar chart of a week's rainfall. Follow the <a href="/docs/quickstart.html#try-the-chart-sample">sample instructions</a> to see what it asks for and run it. The <a href="https://github.com/incyashraj/krate/tree/c46d29500f2b894c89285b04e1b5e2f75184e972/apps/krate-chart">Rust source</a> is available separately.</p>
<p>Full-file SHA-256: <code>3d290c48f74936d5cdb45ceb0ee945bd0f7b5b0b21f87f79917bced3b4ca73c3</code>.</p>
<p>The same bundle passed headless replay and rendering checks on macOS, Windows and Linux in <a href="https://github.com/incyashraj/krate/actions/runs/35454102875">this CI run</a> with source-built runtimes. Each host checks its own reference image:</p>
<ul>
<li>macOS: <a href="https://github.com/incyashraj/krate/blob/7701ad231e841c219f8493ad402b3ea0ff503af4/evidence/golden/macos-latest/chart.png">reference image</a> and <a href="https://github.com/incyashraj/krate/blob/7701ad231e841c219f8493ad402b3ea0ff503af4/evidence/golden/macos-latest/chart.png.json">recorded bundle hash</a>.</li>
<li>Windows: <a href="https://github.com/incyashraj/krate/blob/7701ad231e841c219f8493ad402b3ea0ff503af4/evidence/golden/windows-2022/chart.png">reference image</a> and <a href="https://github.com/incyashraj/krate/blob/7701ad231e841c219f8493ad402b3ea0ff503af4/evidence/golden/windows-2022/chart.png.json">recorded bundle hash</a>.</li>
<li>Linux: <a href="https://github.com/incyashraj/krate/blob/7701ad231e841c219f8493ad402b3ea0ff503af4/evidence/golden/ubuntu-latest/chart.png">reference image</a> and <a href="https://github.com/incyashraj/krate/blob/7701ad231e841c219f8493ad402b3ea0ff503af4/evidence/golden/ubuntu-latest/chart.png.json">recorded bundle hash</a>.</li>
</ul>"""),
            ("What is inside a .krate file?", """<p>The bundle is a ZIP-based application format. Its core entries are <code>manifest.toml</code> and <code>code.wasm</code>, and it can also carry assets, source and SDK material. Apps made in Studio carry their editable source, so you can keep changing them.</p>
<p>The manifest names the app and the access it asks for. The component calls Krate interfaces described in WIT, and the runtime supplies them. Files, the network and other host resources go through the capability model, so the app reaches only what the person allowed.</p>
<p>Read the <a href="https://github.com/incyashraj/krate/tree/main/crates/bundle">bundle implementation</a> and <a href="https://github.com/incyashraj/krate/tree/main/wit">interface definitions</a>. Both are open source.</p>"""),
            ("Small by design", MEASURED),
            ("Verify a shared file yourself", """<p>Copy the <em>same</em> file to each machine, record <code>krate --version</code>, see what it asks for and run it:</p>
<pre class="answer-cmd">krate --version
krate run app.krate --dump-caps
krate run app.krate --prompt</pre>
<p>Compare the full-file SHA-256 on each host: <code>shasum -a 256 app.krate</code> on macOS, <code>sha256sum app.krate</code> on Linux, <code>Get-FileHash app.krate -Algorithm SHA256</code> in Windows PowerShell. Then use the app the same way on each: load data, save, close and reopen.</p>"""),
            ("Bring your project over", """<p>Krate is for real software, from small tools to editors, dashboards and games. Run <code>krate port</code> (beta) on an existing project to see how it maps to Krate's interfaces, or open it in Krate Studio and let the AI bring it over. Electron apps go from hundreds of megabytes to one small file. The <a href="/docs/porting.html">porting guide</a> walks through it, and the <a href="/docs/limits.html">capability reference</a> lists what apps can use today.</p>"""),
        ],
    },
    {
        "slug": "share-an-app-made-with-ai.html",
        "title": "Share an app made with AI as one link | Krate",
        "description": "Describe an app and Krate Studio makes it. Send it as a link and it opens in any browser, phones included, with nothing to install and no account.",
        "h1": "Share an app made with AI as one link",
        "lead": f"Describe your app in Krate Studio, then send it as a link: {LINK_OPENS}. On Mac, Windows and Linux the same <code>.krate</code> file also runs natively, and every app shows what it asks for before it opens.",
        "reviewed": REVIEWED,
        "faq": [
            ("Can I change the app after I send it?", "<p>Yes. Ask for the change, then send the new version. Studio keeps each version, so you can go back if a change goes wrong.</p>"),
            ("Does the person I send it to need my AI tool or an account?", "<p>No. A link opens in their browser with nothing to install and no account. You need a Krate account only to publish a link; opening one never needs one.</p>"),
            ("Which AI tools can make a .krate?", "<p>Krate Studio on your computer works with Claude, Codex, Grok or Gemini. In the browser, Krate's own AI makes your first app.</p>"),
            ("Do I need to know how to code?", "<p>No. You describe the app in plain words and the AI writes the Rust, checks it and packs it. Developers can still open the source and change it by hand.</p>"),
            ("Can I send it by email or chat?", "<p>Yes. A link or a <code>.krate</code> goes anywhere a picture goes: an email, a chat, a shared drive.</p>"),
            ("What if my app needs the internet?", "<p>It declares each host it talks to, and the person opening it sees that list and approves it before the app runs. Hosts it did not declare stay unreachable.</p>"),
        ],
        "entry_actions": [("Make one in Studio", "/studio/"),
                          ("Send a .krate you already have", "#send-it-as-a-link-or-a-file")],
        "entry_note": "Already have a <code>.krate</code> file? Jump to <a href=\"#send-it-as-a-link-or-a-file\">sending it</a>. Made a website or a script with another AI tool? <code>krate port</code> (beta) and the <a href=\"/docs/porting.html\">porting guide</a> bring it over.",
        "sections": [
            ("Make it by describing it", """<p>Open <a href="/studio/">Krate Studio</a> on Mac, Windows or Linux, or <a href="/app/">in your browser</a>, and say what you want in plain words. The AI builds the app, checks it and packs it into one <code>.krate</code>. Then keep changing it by asking: a new button, a dark mode, a CSV export.</p>
<p>Desktop Studio works with Claude, Codex, Grok or Gemini. In the browser, Krate's own AI makes your first app. You do not need to know Rust; the AI writes it.</p>
<p>Developers can do the same from the command line with their own AI agent:</p>
<pre class="answer-cmd">krate doctor
krate ai
krate create "a regex tester with a pattern box and live matches" --agent claude --output regex.krate</pre>
<p><code>krate create --help</code> lists every option.</p>"""),
            ("Test before you send it", """<p>Studio checks every build and opens the app for you to try. Click through it, resize the window, type a bad input. From the command line, see what it asks for and run it:</p>
<pre class="answer-cmd">krate run regex.krate --dump-caps
krate run regex.krate --prompt</pre>
<p>The first command shows the permission request without running anything. The second opens the app after you review it.</p>"""),
            ("Send it as a link or a file", f"""<p><strong>As a link.</strong> Publish the app and you get a link like <code>https://hub.krate.tech/run/&lt;id&gt;</code>. Paste it into a chat or an email; {LINK_OPENS}. Publishing needs a Krate account; opening never does.</p>
<pre class="answer-cmd">krate publish regex.krate</pre>
<p><strong>As a file.</strong> Once you have <a href="#test-before-you-send-it">tried the app</a>, send the <code>.krate</code> itself by email, chat or a shared drive. One file, from 37 KB. On Mac, Windows and Linux, the Krate runtime opens it natively; <a href="/open/">here is what the person receiving it does</a>. They never need your AI account, your source or a Rust toolchain.</p>"""),
            ("What the person you send it to sees", """<p>Before the app opens, they see exactly what it asks for and decide. It starts with no access to their files or the network, hosts it did not declare stay unreachable, and it keeps its data in storage of its own. See <a href="/run-ai-generated-code-safely.html">how permissions work</a> and <a href="/portable-desktop-app-format.html">how one file opens on every screen</a>.</p>"""),
        ],
    },
    {
        "slug": "run-ai-generated-code-safely.html",
        "title": "Run an AI-built app safely: see what it can touch | Krate",
        "description": "A Krate app starts with no access to your files or the network. Before it opens, you see exactly what it asks for and decide.",
        "h1": "Run an AI-built app safely: see what it can touch first",
        "lead": "A Krate app starts with no access to your files or the network, and before it opens, from a link or from the file, you see exactly what it asks for and decide. An AI-built app reaches only what you allow: hosts it did not declare stay unreachable, and each app keeps its own storage.",
        "reviewed": REVIEWED,
        "faq": [
            ("Where does an app keep its data?", "<p>In storage of its own, kept per app, which it declares in its permission request. It gets no folder on your disk unless you grant one or pick a file for it.</p>"),
            ("What can a Krate app do without asking?", "<p>Only harmless things: open its window, draw, play sound, read the clock and language settings, and show a dialog. Files, the network, the camera, the microphone and its own storage must be declared and approved.</p>"),
            ("Is the permission screen enforced?", "<p>Yes. The runtime checks protected calls against what you allowed, and hosts the app did not declare stay unreachable. Grant only what the task needs.</p>"),
            ("Can an app ask for more access while it runs?", "<p>No. Everything an app can reach is declared in its manifest and shown before it opens. A new need comes in a new version, and you see that request before the new version opens.</p>"),
            ("Can I see what an app asks for without running it?", "<p>Yes. <code>krate run app.krate --dump-caps</code> prints the request without granting anything or starting the app.</p>"),
            ("Does Krate replace my operating system's protections?", "<p>It adds to them. Your operating system still checks the Krate runtime itself; Krate then decides what each app inside it may touch.</p>"),
        ],
        "entry_actions": [("Inspect an app's permissions", "#read-the-permission-request-before-it-runs"),
                          ("Install the runtime", "/docs/quickstart.html#get-krate")],
        "sections": [
            ("Read the permission request before it runs", """<p>Every Krate app lists what it wants in its manifest. You see that list before the app opens. From the command line you can read it without running anything:</p>
<pre class="answer-cmd">krate run app.krate --dump-caps</pre>
<p>Check the file scope, network hosts and devices against what the app is for. If a document viewer asks to upload data, you see it before a single line of it runs.</p>"""),
            ("Decide each grant when it opens", """<pre class="answer-cmd">krate run app.krate --prompt</pre>
<p>The runtime checks host operations against what you granted for that session. Refuse a capability the app requires and the app does not launch. Grant only what the task needs, and keep <code>--auto-grant</code> for apps you already know.</p>"""),
            ("Where enforcement happens", """<p>The app is a WebAssembly component. It has no direct line to your operating system: it calls Krate's WIT interfaces, and the native runtime implements them. Import validation and host-side capability checks enforce the boundary, and the permission dialog belongs to the runtime, not to the app.</p>
<p>Explore the <a href="https://github.com/incyashraj/krate/tree/main/wit">interface definitions</a>, the <a href="https://github.com/incyashraj/krate/tree/main/crates/policy">policy implementation</a> and the <a href="/docs/architecture.html">architecture guide</a>. All of it is open source.</p>"""),
            ("A practical review checklist", """<ol><li>Get the app from someone you know, or check its publisher.</li><li>Record its version and hash, and compare them with the ones the publisher shared.</li><li>Read the permission request before it opens.</li><li>Grant the narrowest access that does the job.</li><li>Recheck the request when a new version arrives.</li></ol>
<p>Found a problem in Krate itself? The project's <a href="https://github.com/incyashraj/krate/blob/main/SECURITY.md">security reporting policy</a> says how to report it.</p>"""),
        ],
    },
    {
        "slug": "desktop-app-distribution.html",
        "title": "Krate vs Electron vs Tauri: what do you ship? | Krate",
        "description": "Electron and Tauri ship a package for each OS. Krate ships one .krate file that runs natively on every desktop and opens from a link in any browser.",
        "h1": "Krate vs Electron vs Tauri: what do you ship?",
        "lead": "Electron and Tauri ship a separate package for each operating system; Krate ships one <code>.krate</code> file that runs natively on Mac, Windows and Linux and opens from a link in any browser, phones included. You build once, send once, and everyone gets the same file.",
        "reviewed": REVIEWED,
        "faq": [
            ("Which one should I pick?", "<p>Pick Krate when you want one file for every screen, a link anyone can open, and permissions the user approves. Electron and Tauri suit teams that want to keep a web codebase and ship a package per OS.</p>"),
            ("What do users install with each?", "<p>With Electron and Tauri, each user installs your app's own package for their system. With Krate, nothing to open a link; to run it natively, the Krate runtime once, and then every <code>.krate</code> opens from the file.</p>"),
            ("Can I send the same file to every system?", "<p>Yes, with Krate. Electron and Tauri produce a package per operating system; a <code>.krate</code> is the same bytes on macOS, Windows and Linux.</p>"),
        ],
        "entry_actions": [("Try the same-file example", "/docs/quickstart.html#try-the-chart-sample"),
                          ("Bring your project over", "/docs/porting.html")],
        "sections": [
            ("One app, two shipping workflows", """<p>When you release an update, what has to reach each user? In the usual Electron or Tauri workflow, a separate build for their platform. With Krate, the same file for everyone.</p>
<figure class="shipping-flow">
<figcaption>Typical desktop packaging, compared with Krate. Read each row from build to delivery.</figcaption>
<div class="shipping-lane"><h3>Electron / Tauri</h3><ol>
<li><span>1. Develop</span>Shared application code</li>
<li><span>2. Package</span>Platform-specific applications</li>
<li><span>3. Deliver</span>macOS package<br>Windows package<br>Linux package</li>
</ol></div>
<div class="shipping-lane shipping-lane-krate"><h3>Krate</h3><ol>
<li><span>1. Make</span>Describe it in Studio, or write it with the SDK</li>
<li><span>2. Package</span>One <code>app.krate</code> file</li>
<li><span>3. Deliver</span>A link for any browser, and the same file for Mac, Windows and Linux</li>
</ol></div>
</figure>
<p>A link opens the app in any modern browser with nothing to install. On a desktop, the Krate runtime runs the same file natively; it is installed once and shared by every Krate app.</p>
<p>See Electron's <a href="https://www.electronjs.org/docs/latest/tutorial/application-distribution">application packaging</a> and Tauri's <a href="https://tauri.app/distribute/">distribution formats</a>. Electron's <code>app.asar</code> can package application source, but it is delivered inside a platform-specific Electron distribution; it is not a standalone cross-OS executable.</p>"""),
            ("Compare the distribution boundary", """<div class="answer-table-wrap" role="region" aria-label="Distribution comparison" tabindex="0"><table class="answer-table">
<caption>Desktop distribution models, reviewed 20 September 2026</caption>
<thead><tr><th scope="col">Question</th><th scope="col">Electron</th><th scope="col">Tauri</th><th scope="col">Krate</th></tr></thead>
<tbody>
<tr><th scope="row">Application artifact</th><td>Platform-specific packaged application.</td><td>Platform-specific application bundle or installer.</td><td>One .krate file for every system, and a link that opens it in any browser.</td></tr>
<tr><th scope="row">UI/runtime model</th><td>Chromium and Node.js in Electron's process model.</td><td>Web frontend in an OS WebView with a Rust backend.</td><td>WebAssembly app drawn natively by the Krate runtime, with no browser or WebView inside.</td></tr>
<tr><th scope="row">Recipient prerequisite</th><td>The packaged application and its platform requirements.</td><td>The packaged application and platform/WebView requirements.</td><td>Nothing, to open a link. The Krate runtime, installed once, to run it natively.</td></tr>
<tr><th scope="row">Existing app migration</th><td>Fits browser/Node-based applications.</td><td>Fits web UI with Rust/native integration.</td><td><code>krate port</code> (beta) and Studio's AI bring the project over to Krate's APIs.</td></tr>
</tbody></table></div>
<p>Electron's <a href="https://www.electronjs.org/docs/latest/tutorial/distribution-overview">distribution guide</a> covers packaging, signing, publishing and updates; its <a href="https://www.electronjs.org/docs/latest/tutorial/process-model">process model</a> explains Chromium and Node. Tauri documents <a href="https://tauri.app/distribute/">platform-specific distribution</a> and its <a href="https://tauri.app/concept/architecture/">WebView/Rust architecture</a>. Tauri does not bundle Chromium like Electron.</p>"""),
            ("Can I send exactly the same file to all three systems?", """<p>Yes, with Krate. Build once and send the same .krate file to Mac, Windows and Linux users, or publish it and send one link. There is no separate application build per OS.</p>
<p>Try <a href="/docs/quickstart.html#try-the-chart-sample">the downloadable Chart example</a>. Its <a href="/portable-desktop-app-format.html#try-one-file-tested-on-all-three-systems">source, full-file checksum and three-OS test evidence</a> let you check it yourself. No Rust toolchain or AI account is needed to run the file.</p>"""),
            ("Smaller, measured", MEASURED),
            ("Bring your project over", """<p>Run <code>krate port ./my-project</code> (beta) to see how your app maps to Krate's interfaces, or open the project in Krate Studio and let the AI bring it over. Electron apps go from hundreds of megabytes to one small file. Start with one representative feature, then follow the <a href="/docs/porting.html">porting checklist</a>.</p>"""),
            ("What happens when the app changes?", """<p>Build the next .krate and publish it or send it. There is no app store in the way and no build per OS to redo. Runtime updates arrive separately, so your app stays one small file.</p>"""),
        ],
    },
]


# Shipping-led pages (K-SEO, 6 October 2026). Every page answers in its lead,
# names the other tools fairly and links official sources for their facts.
PAGES += [
    {
        "slug": "how-to-distribute-a-desktop-app.html",
        "title": "How to distribute a desktop app: ship one file | Krate",
        "description": "Skip the build, signature and download per OS: ship one .krate file that runs natively on macOS, Windows and Linux and opens from a link anywhere.",
        "h1": "How to distribute a desktop app: ship one file",
        "lead": f"Ship it as one <code>.krate</code> file: the same file runs natively on macOS, Windows and Linux, and sent as a link, {LINK_OPENS}. There is no separate build, signature or download for each operating system, and the app asks before it touches anyone's files or the network.",
        "reviewed": REVIEWED,
        "entry_actions": [("Compare the two routes", "#the-usual-route-step-by-step"),
                          ("Try a .krate file", "/docs/quickstart.html#try-the-chart-sample")],
        "sections": [
            ("The usual route, step by step", f"""<ol>
<li><strong>Build once per operating system.</strong> Electron and Tauri share your code, then produce a separate package for macOS, Windows and Linux. In practice that means a build machine or CI runner for each system, and macOS signing needs a Mac.</li>
<li><strong>Often build once per processor.</strong> Apple silicon and Intel Macs, x64 and ARM64 Windows, x86_64 and arm64 Linux each need native code built for them, or a universal build that carries both.</li>
<li><strong>Sign and notarize.</strong> For direct downloads, macOS expects a Developer ID signature and Apple notarization, which needs a paid Apple Developer Program membership. On Windows, an unsigned download shows "Windows protected your PC", and even a newly signed app can be flagged as unrecognized until it builds reputation.</li>
<li><strong>Package and host each one.</strong> A DMG or app bundle for macOS, an installer for Windows, and AppImage, deb, RPM, Flatpak or Snap for Linux. Each gets its own download link and its own update path.</li>
</ol>
<p>After all that, a conventional desktop program usually runs with the same access to files and the network as the person who opened it, unless the platform sandboxes it, as the Mac App Store does. A signature says who made the app. It does not limit which of your files the app can read.</p>
<p>Sources: Electron's <a href="{EL_SIGN}">code signing guide</a>, Tauri's <a href="{TA_DIST}">distribution guide</a> and Microsoft's <a href="{MS_SS}">SmartScreen reputation guide</a>.</p>"""),
            ("The Krate route", f"""<ol>
<li><strong>Make the app.</strong> Describe it in <a href="/studio/">Krate Studio</a> and the AI builds it, or write it in Rust with the Krate SDK. Studio can also bring over a project you already have.</li>
<li><strong>Pack one file.</strong> The result is one <code>.krate</code>, from 37 KB: a manifest that names the access the app wants, a WebAssembly component that holds the app, and optionally its assets and source. There is no machine code inside, so there is nothing to build per processor.</li>
<li><strong>Send it like a picture.</strong> Publish it and send the link, or email it, drop it in a chat or a shared folder. Everyone gets the same bytes.</li>
<li><strong>They open it anywhere.</strong> From a link, {LINK_OPENS}. On Mac, Windows and Linux, the Krate runtime runs it natively; Krate for Mac is signed and notarized by Apple, and after the first install every new app is just its own file.</li>
<li><strong>They decide what it may touch.</strong> An app starts with no access to files or the network and asks for what it needs. The runtime checks protected calls against what was allowed. See <a href="/run-ai-generated-code-safely.html">how to see what an app asks for</a>.</li>
</ol>"""),
            ("What you stop doing", """<p>You stop producing a separate package for each operating system and processor. You stop signing and notarizing each app for each platform, hosting a download per system, and asking people to run an installer that gets their full permissions. You ship one file, or one link, and spend the time on the app.</p>"""),
            ("Smaller, measured", MEASURED),
            ("Check it yourself", f"""<p>Download <a href="{CHART}">chart.krate</a>, one file of 11,455 bytes with SHA-256 <code>3d290c48f74936d5cdb45ceb0ee945bd0f7b5b0b21f87f79917bced3b4ca73c3</code>. Follow <a href="/docs/quickstart.html#try-the-chart-sample">the quickstart</a> to see what it asks for and run it on any of the three systems.</p>
{THREE_OS_REPLAY}"""),
        ],
        "faq": [
            ("Can one file run on Windows, macOS and Linux?", "<p>A normal native executable can't, because each system uses its own executable format. A <code>.krate</code> can: it holds a WebAssembly component that the Krate runtime runs natively on each system, and a link opens it in any browser too.</p>"),
            ("Do the people I send it to need to install anything?", '<p>Not to open a link: it runs in their browser, phones included, with no account. To run it natively, they install the Krate runtime once from the <a href="/download/">download page</a>, and every Krate app after that is just a file.</p>'),
            ("Do I need a code signing certificate to ship a .krate?", '<p>No. There is no package per operating system to sign and notarize, because a <code>.krate</code> isn\'t one. You can still sign a <code>.krate</code> with your own key so people can check it came from you; see <a href="/desktop-app-code-signing.html">code signing and Krate</a>.</p>'),
            ("Can I ship an app I made with AI this way?", '<p>Yes. Make it in Krate Studio with the AI you already use, or bring an existing project into Studio. See <a href="/share-an-app-made-with-ai.html">how to share an app made with AI</a>.</p>'),
        ],
    },
    {
        "slug": "krate-vs-electron.html",
        "title": "Krate vs Electron: one native file, no browser inside",
        "description": "Electron puts Chromium and Node.js in every app and ships a package per OS. Krate ships one small .krate file that runs natively and opens from a link.",
        "h1": "Krate vs Electron: one small native file with no browser inside",
        "lead": "Krate ships your app as one small <code>.krate</code> file that runs natively on Mac, Windows and Linux and opens from a link in any browser, phones included; Electron puts Chromium and Node.js inside every app and ships a package for each operating system. On the same 50,000-line notes workload, measured on 25 August 2026 on an Apple M4, Krate's Notes app used 13x less memory and opened 2.6x faster than MarkText 0.17.1, an Electron editor doing the equivalent work.",
        "reviewed": REVIEWED,
        "entry_actions": [("See the comparison", "#side-by-side"),
                          ("Bring your Electron app over", "/docs/porting.html")],
        "sections": [
            ("Side by side", comparison_table("Electron and Krate, reviewed 6 October 2026 against Electron's official docs", "Electron and Krate compared",
                ["Question", "Electron", "Krate"], [
                ["What you ship", "A packaged app for each operating system, each carrying its own Chromium and Node.js.", "One <code>.krate</code> file for every system, and a link that opens it in any browser."],
                ["App code", "HTML, CSS and JavaScript in Chromium, with Node.js in the main process.", "A WebAssembly component that calls Krate's interfaces. Describe it in Studio and the AI writes it, or write Rust yourself."],
                ["Builds per OS and CPU", "One package per target platform.", "One file. Krate builds the native runtimes."],
                ["Signing", "Sign for Windows and macOS, and notarize for macOS, for each release.", "No per-OS signing of the app file. Optional Krate signature with your own key."],
                ["Default access", "Renderers are sandboxed by default since Electron 20. The main process is not sandboxed and has Node.js.", "The app starts with no file or network access. The person running it grants or refuses each request."],
                ["Platforms", "Windows, macOS and Linux.", "macOS, Windows and Linux natively, and any modern browser from a link, phones included."],
            ]) + f"""
<p>Electron facts: <a href="{EL_PROC}">process model</a>, <a href="{EL_SANDBOX}">process sandboxing</a>, <a href="{EL_SIGN}">code signing</a> and <a href="{EL_DIST}">distribution overview</a>.</p>"""),
            ("Measured against an Electron editor", MEASURED),
            ("What one file changes for shipping", f"""<p>With Electron, each release is a set of platform builds that you sign, notarize and host. With Krate, each release is one new file. Send the link and {LINK_OPENS}. On a desktop, the Krate runtime opens the file natively, and because every Krate app shares that runtime, a second or third app never brings another copy of a browser.</p>"""),
            ("Who decides what the app can touch", """<p>In Electron, the developer decides what the main process exposes to the sandboxed renderers, and the main process itself runs with the user's permissions. That is a sound design for code you wrote, but the person running the app has no say in it.</p>
<p>In Krate, the app lists what it wants in its manifest and the person running it approves or refuses, before it opens. The runtime checks protected calls against what was allowed, and hosts the app did not declare stay unreachable.</p>"""),
            ("Moving an Electron app to Krate", """<p>Start with a scan. It reads your source without building or running it and shows how each part maps to Krate:</p>
<pre class="answer-cmd">krate port ./my-electron-app</pre>
<p><code>krate port</code> is in beta. Krate Studio can do the move with AI: it rewrites the app against the Krate SDK and shows its plan first. Electron apps go from hundreds of megabytes to one small file. Begin with one representative feature and read the <a href="/docs/porting.html">porting guide</a>.</p>"""),
        ],
        "faq": [
            ("How do I move an Electron app to Krate?", "<p>Run <code>krate port</code> (beta) on the project, or open it in Krate Studio: the AI rewrites it against Krate's interfaces and shows its plan first. The result is one small <code>.krate</code> file in place of a package per OS.</p>"),
            ("Do Krate users need Chromium?", "<p>No. A Krate app runs natively, with no browser engine inside, and every Krate app shares one runtime. From a link, it opens in whatever browser the person already has.</p>"),
            ("Do I need to know Rust?", "<p>No. Describe the app in Krate Studio and the AI writes the Rust for you. Developers who like Rust can write it by hand with the Krate SDK.</p>"),
            ("Which one should I pick?", "<p>Pick Krate when you want one small file that opens anywhere, uses a fraction of the memory and asks before it touches anything. Electron suits a team with a large web codebase built on Node.js packages that wants to keep that code exactly as it is.</p>"),
        ],
    },
    {
        "slug": "krate-vs-tauri.html",
        "title": "Krate vs Tauri: one file, every desktop | Krate",
        "description": "Tauri uses the system WebView and builds a bundle per OS. Krate ships one .krate file with no WebView that runs natively and opens from a link anywhere.",
        "h1": "Krate vs Tauri: one file for every desktop and every browser",
        "lead": "Krate ships one <code>.krate</code> file with no WebView that runs natively on Mac, Windows and Linux and opens from a link in any browser, phones included; Tauri uses the system WebView and builds a separate bundle for each operating system. With Krate, the person running the app approves what it may touch before it opens.",
        "reviewed": REVIEWED,
        "entry_actions": [("See the comparison", "#side-by-side"),
                          ("Try a .krate file", "/docs/quickstart.html#try-the-chart-sample")],
        "sections": [
            ("Side by side", comparison_table("Tauri and Krate, reviewed 6 October 2026 against Tauri's official docs", "Tauri and Krate compared",
                ["Question", "Tauri", "Krate"], [
                ["What you ship", "An installer or bundle per platform: DMG or app bundle, a Windows installer, AppImage, deb, RPM, Flatpak or Snap.", "One <code>.krate</code> file for macOS, Windows and Linux, and a link that opens it in any browser."],
                ["Interface", "Your web frontend in the system WebView: WebView2, WKWebView or webkit2gtk.", "Krate's own UI, drawn natively by the runtime. No WebView."],
                ["App logic", "Rust commands, with plugins in Rust, Swift or Kotlin.", "A WebAssembly component in Rust, written by you or by Studio's AI."],
                ["Permissions", "Capability files the developer writes grant or deny commands per window.", "The app requests access in its manifest; the person running it grants or refuses."],
                ["Signing", "Required on most platforms. Direct macOS downloads also need notarization.", "No per-OS signing of the app file. Optional Krate signature with your own key."],
                ["Platforms", "Windows, macOS, Linux, Android and iOS.", "macOS, Windows and Linux natively, and any modern browser from a link, phones included."],
            ]) + f"""
<p>Tauri facts: <a href="{TA_DIST}">distribution</a>, <a href="{TA_CAPS}">capabilities</a> and <a href="{TA_WEBVIEW}">WebView versions</a>.</p>"""),
            ("Who holds the permissions", f"""<p>Tauri's capabilities limit what the frontend in the WebView may call. They are chosen by the developer, and <a href="{TA_CAPS}">Tauri's own docs</a> say they do not protect against malicious or insecure Rust code in the app itself. They protect users from a compromised frontend.</p>
<p>In Krate, the whole app is the guest. It has no direct line to the operating system; it calls Krate interfaces, and the runtime checks protected calls against what the person running it allowed.</p>"""),
            ("Shipping a release", f"""<p>With Tauri, a release means building on each platform, usually in a CI job per system, signing for each platform, and hosting and updating each package. With Krate, a release is one new <code>.krate</code> file. Publish it and send the link, and {LINK_OPENS}; on a desktop, the same file runs natively.</p>"""),
            ("Moving a Tauri app to Krate", """<p>Both are Rust, so a lot comes with you. Run the scan, which reads your source without building it:</p>
<pre class="answer-cmd">krate port ./my-tauri-app</pre>
<p><code>krate port</code> is in beta. Pure Rust logic often carries over as it is; the WebView interface moves to Krate's widgets, canvas and host interfaces, and Krate Studio's AI can do that part for you. See the <a href="/docs/porting.html">porting guide</a>.</p>"""),
        ],
        "faq": [
            ("Which one should a Rust developer pick?", "<p>Pick Krate when you want one file for every desktop and every browser, no WebView, and permissions the user approves before the app opens. Tauri suits a team that wants to keep a web frontend and ship a bundle per OS.</p>"),
            ("What does the person I send it to install?", "<p>With Tauri, your app's own installer or bundle for their system. With Krate, nothing to open a link; to run it natively, the Krate runtime once, and then every <code>.krate</code> opens from the file.</p>"),
            ("Is Krate built on Tauri?", "<p>No. Krate is its own runtime, built on Wasmtime. It does not use a WebView.</p>"),
            ("How does a Tauri app move to Krate?", "<p>With a port. Rust logic often carries across, and <code>krate port</code> (beta) and Krate Studio's AI rewrite the WebView interface against Krate's interfaces.</p>"),
            ("Which one makes smaller downloads?", "<p>Krate apps are tiny: Krate's Notes app is one 36.6 KiB file, and a link needs nothing installed. The native runtime is installed once and shared by every Krate app, so each new app is only its own file.</p>"),
        ],
    },
    {
        "slug": "desktop-app-or-web-app.html",
        "title": "Desktop app or web app? Ship one file that is both | Krate",
        "description": "A .krate opens from a link in any browser like a web app and runs natively on Mac, Windows and Linux like a desktop app. No server to run.",
        "h1": "Desktop app or web app? Ship one file that is both",
        "lead": "With Krate you do not have to choose: a <code>.krate</code> opens from a link in any browser, phones included, like a web app, and runs natively on Mac, Windows and Linux like a desktop app. You run no server for either, and the app asks before it touches anyone's files or the network.",
        "reviewed": REVIEWED,
        "entry_actions": [("See the comparison", "#side-by-side"),
                          ("How to distribute a desktop app", "/how-to-distribute-a-desktop-app.html")],
        "sections": [
            ("Side by side", comparison_table("A hosted web app and a Krate app, reviewed 27 October 2026", "Web app and Krate app compared",
                ["Question", "Hosted web app", "Krate app"], [
                ["What people get", "A page in a browser tab, or an installed web app where the browser supports it.", "The app in their browser from a link, and a native window on Mac, Windows and Linux."],
                ["What you run", "A server or hosting account, for as long as people use the app.", "No server. Krate's hub serves the link, and the native app runs from the file."],
                ["Offline", "Only with extra work, such as a service worker.", "The native app works without a connection unless it needs the network itself."],
                ["Local files", f'Depends on the browser. The File System Access picker is <a href="{MDN_FSA}">not available in all major browsers</a>.', "Through a file dialog or a folder the person grants."],
                ["Updates", "Every visitor gets the new version on reload.", "You publish or send the new version."],
                ["Phones", "Yes.", "Yes, from a link in the phone's browser."],
                ["What people install", "Nothing.", "Nothing, to open a link. The Krate runtime, once, to run it natively."],
            ])),
            ("No server to run", """<p>A hosted app needs a domain, a hosting bill, uptime, sign-in and storage for user data, and it keeps people's data on your server, which brings privacy duties. A Krate app keeps its saved data on the person's computer, in storage of its own, unless they grant it network access. If it talks to a service, that service is yours to run; the app itself needs no hosting.</p>"""),
            ("Apps made with Lovable, Bolt, v0 and similar tools", """<p>These tools mostly produce web apps you have to host, or wrap per OS with Electron or Tauri. Krate gives you a better path: open the project in Krate Studio and the AI brings it over, rewriting it against Krate's interfaces and showing its plan first. You end up with one file that opens from a link and runs natively. Or skip the detour and describe the app to Studio from the start. See <a href="/share-an-app-made-with-ai.html">how to share an app made with AI</a>.</p>"""),
        ],
        "faq": [
            ("Does a Krate app need a server?", "<p>No. Krate's hub serves the link, and the native app runs on the person's computer from the file. If it calls an API you host, that service still runs, but the app itself needs no server and no domain.</p>"),
            ("What does the person I send it to need?", "<p>Nothing, to open a link: it runs in their browser with no account. To run it natively, the Krate runtime, installed once for their system.</p>"),
            ("Can I still use the web from a Krate app?", "<p>Yes. A Krate app can call APIs over HTTP and WebSockets for the hosts it declares, which the person approves before it runs.</p>"),
            ("Can I turn my website into a desktop app?", "<p>Often, yes. <code>krate port</code> (beta) reads your project and shows what maps across, and Krate Studio's AI rewrites it against Krate's interfaces, so you get one file that runs natively and opens from a link, with no per-OS package to build or sign.</p>"),
            ("Do Krate apps need an internet connection?", "<p>The native app does not, unless it uses the network itself, and then only to the hosts the person allows. A link needs a connection to load, like any page.</p>"),
            ("Can people open a .krate on their phone?", "<p>Yes. Send a link and it opens in the phone's browser, with nothing to install and no account.</p>"),
        ],
    },
    {
        "slug": "desktop-app-code-signing.html",
        "title": "Ship a desktop app without code signing for every OS",
        "description": "Native installers need an Apple Developer ID, notarization and a Windows signature. A Krate app is one link or one .krate file, with nothing per OS to sign.",
        "h1": "Ship a desktop app without code signing for every OS",
        "lead": "A Krate app needs no code signing per operating system, because you send a link that opens in any browser or one <code>.krate</code> file that the Krate runtime opens natively. Native installers need an Apple Developer ID and notarization on macOS and a trusted signature on Windows; a Krate app skips that whole step.",
        "reviewed": REVIEWED,
        "entry_actions": [("Sign a .krate with your own key", "#signing-a-krate-with-your-own-key"),
                          ("Install Krate", "/download/")],
        "sections": [
            ("What signing takes for a native app", f"""<ul>
<li><strong>macOS:</strong> an Apple Developer Program membership with an annual fee, Xcode on a Mac, signing certificates, then an upload to Apple for notarization.</li>
<li><strong>Windows:</strong> an unsigned download shows "Windows protected your PC" and the user must choose to run it anyway. A file signed with a new certificate can still be flagged as unrecognized until it builds reputation, and an EV certificate no longer skips that. On Windows 11, Smart App Control can block unsigned files that have no positive reputation.</li>
<li><strong>Linux:</strong> package and repository signing depends on the format and the store.</li>
</ul>
<p>Sources: Electron's <a href="{EL_SIGN}">code signing guide</a> and Microsoft's <a href="{MS_SS}">SmartScreen reputation guide</a>.</p>"""),
            ("What a signature tells your users", """<p>A signature identifies the publisher and shows the file has not been changed since it was signed. It does not limit what the program can reach once it runs: a signed installer still gets the same access to files and the network as the person who opened it.</p>"""),
            ("How it works with Krate", f"""<p><strong>From a link,</strong> {LINK_OPENS}. There is no installer of yours for anyone to approve.</p>
<p><strong>Natively,</strong> your app is a file the Krate runtime opens, not a program the operating system launches, so there is no per-OS package of yours to sign or notarize. Krate for Mac is signed and notarized by Apple. When someone opens your app, Krate shows what it asks for, and the app gets only what they allow.</p>"""),
            ("Signing a .krate with your own key", """<p>You can sign a bundle so recipients can check it came from you and has not changed. The key is an Ed25519 key you create; it stays on your machine, and signing and checking work offline with no account:</p>
<pre class="answer-cmd">krate sign app.krate --key publisher.key --generate-key --namespace com.example.app</pre>
<p><code>--generate-key</code> writes a new key the first time and refuses to overwrite one. <code>krate sign --help</code> lists every option. Every release you sign is tied to your key.</p>"""),
            ("What you get back", """<ul>
<li>No Apple Developer Program fee and no notarization queue for your app.</li>
<li>No certificate to buy, renew and keep safe for each platform.</li>
<li>No reputation to build per release before people can open it.</li>
<li>One file or one link per release, the same for everyone.</li>
</ul>"""),
        ],
        "faq": [
            ("Does a Krate app need its own Apple Developer ID?", "<p>No. The app is a file that the Krate runtime opens, and Krate for Mac is signed and notarized by Apple. You do not sign or notarize the <code>.krate</code> for each system.</p>"),
            ("Can I distribute a Windows app without a code signing certificate?", "<p>A native <code>.exe</code> without one shows a SmartScreen warning, and some managed or Smart App Control machines block it. With Krate there is no <code>.exe</code> of yours: you send a link that opens in the browser, or a <code>.krate</code> file the Krate runtime opens.</p>"),
            ("Do I need an Apple Developer account to share a Mac app?", "<p>For a native Mac app downloaded from the web, in practice yes: notarization needs a Developer ID from the paid Apple Developer Program. A <code>.krate</code> is opened by Krate for Mac, which is signed and notarized, so you don't need your own Apple account to share one.</p>"),
            ("Is a signed app safe to run?", "<p>A signature tells you who made it and that it was not changed. It does not limit what the app can do once it runs. Krate adds what a signature leaves out: the app asks before it gets your files or the network.</p>"),
        ],
    },
    {
        "slug": "desktop-app-shipping-faq.html",
        "title": "Shipping a desktop app in 2026: straight answers | Krate",
        "description": "Straight answers about shipping an app in 2026: one file for every desktop, a link for every browser, code signing, Electron and Tauri, and apps built with AI.",
        "h1": "Shipping a desktop app in 2026: questions and straight answers",
        "lead": "AI solved building the app, and Krate solves shipping it: one <code>.krate</code> file that runs natively on Mac, Windows and Linux and opens from a link in any browser, phones included. Each answer below starts with the answer, then links to the longer page.",
        "reviewed": REVIEWED,
        "entry_actions": [("How to distribute a desktop app", "/how-to-distribute-a-desktop-app.html"),
                          ("Install Krate", "/download/")],
        "entry_note": "These answers describe Krate 0.5.4, the current release.",
        "sections": [],
        "faq": [
            ("How do I distribute a desktop app to Windows, macOS and Linux?", '<p>Ship one <code>.krate</code> file: it runs natively on all three, and a link opens it in any browser. The usual alternative is to build, sign and host a package for each system. <a href="/how-to-distribute-a-desktop-app.html">Both routes, step by step</a>.</p>'),
            ("Can I ship one app file to Windows, Mac and Linux?", '<p>Yes, as a <code>.krate</code>: the same bytes run natively on all three, and the same app opens from a link in any browser. A normal native executable cannot do this. <a href="/portable-desktop-app-format.html">How the format works</a>.</p>'),
            ("Can people open my app on their phone?", '<p>Yes. Send a link and it opens in the phone\'s browser, with nothing to install and no account. <a href="/share-an-app-made-with-ai.html">Sending an app as a link</a>.</p>'),
            ("What is a good Electron alternative?", '<p>Krate, if you want one small file for every screen: it runs natively with no browser inside and opens from a link anywhere. Tauri, Wails and Neutralino keep a web frontend in the system WebView, and Flutter and Qt draw their own interface. <a href="/krate-vs-electron.html">Krate vs Electron</a>.</p>'),
            ("Is there a Tauri alternative that doesn't need a build per OS?", '<p>Krate. Tauri builds an installer or bundle for each operating system; Krate ships one file and does the per-OS work once, in runtimes Krate maintains. <a href="/krate-vs-tauri.html">Krate vs Tauri</a>.</p>'),
            ("How do I share an app I built with AI?", '<p>Make it in Krate Studio and send the link or the <code>.krate</code> file. If another AI tool built you a website, open the project in Krate Studio and bring it over. <a href="/share-an-app-made-with-ai.html">Sharing an app made with AI</a>.</p>'),
            ("Can I ship a cross-platform app without code signing?", '<p>Yes, with Krate. You send a link or one <code>.krate</code> file, so there is no package per OS to sign, while native installers effectively need signing on macOS and Windows. <a href="/desktop-app-code-signing.html">Code signing and Krate</a>.</p>'),
            ("How can I run an untrusted desktop app more safely?", '<p>For an unknown native program, use the strongest isolation you have, such as a virtual machine or Windows Sandbox. A Krate app starts with no file or network access and shows what it asks for before it opens; <code>krate run app.krate --dump-caps</code> prints the request without running it. <a href="/run-ai-generated-code-safely.html">Seeing what an app asks for</a>.</p>'),
            ("Is there a portable app format that runs on every OS?", '<p>Yes: a <code>.krate</code> runs natively on macOS, Windows and Linux and opens from a link in any modern browser, phones included. AppImage is Linux only, Cosmopolitan\'s Actually Portable Executable runs one executable on several systems, mainly for command-line programs, and Java apps need a Java runtime. <a href="/portable-desktop-app-format.html">The .krate format</a>.</p>'),
            ("Is there a WebAssembly runtime for desktop GUI apps?", '<p>Yes. Krate runs desktop apps compiled to WebAssembly components, with windows, widgets, a 2D canvas, files, storage and networking behind permissions. It embeds Wasmtime, while general runtimes such as Wasmtime, Wasmer and WasmEdge focus on servers and command-line programs. <a href="/docs/architecture.html">Krate\'s architecture</a>.</p>'),
            ("Do people need to install anything to open a .krate?", '<p>No, not to open a link: it runs in any modern browser with no account. To run apps natively, they <a href="/download/">install Krate</a> once; read <a href="/open/">what to do when someone sends you a .krate</a>.</p>'),
            ("Is Krate free and open source?", '<p>Yes. The runtime, the <code>.krate</code> format and the CLI are open source under MIT OR Apache-2.0, and Krate Studio is under the Business Source License 1.1. <a href="https://github.com/incyashraj/krate#license">Licence details</a>.</p>'),
            ("Can I bring an existing app to Krate?", '<p>Yes. <code>krate port</code> (beta) brings a project over, and Krate Studio can do it with AI. Electron apps go from hundreds of megabytes to one small file; the <a href="/docs/porting.html">porting guide</a> shows how.</p>'),
        ],
    },
]


# Comparison pages for "alternatives" and "frameworks" searches (visibility
# instructions T5, rows 4 to 6). Every fact about another tool comes from its
# official docs, read on 6 October 2026 and linked beside the table.
EL_HOME = "https://www.electronjs.org/docs/latest/"
EL_FORGE = "https://www.electronjs.org/docs/latest/tutorial/forge-overview"
TA_SEC = "https://v2.tauri.app/security/"
TA_WINSIGN = "https://v2.tauri.app/distribute/sign/windows/"
TA_START = "https://v2.tauri.app/start/"
WA_INTRO = "https://wails.io/docs/introduction"
WA_XPLAT = "https://wails.io/docs/guides/crossplatform-build"
WA_WIN = "https://wails.io/docs/guides/windows"
NE_DOCS = "https://neutralino.js.org/docs/"
NE_DIST = "https://neutralino.js.org/docs/distribution/overview/"
NE_CONF = "https://neutralino.js.org/docs/configuration/neutralino.config.json/"
FL_DESKTOP = "https://docs.flutter.dev/platform-integration/desktop"
FL_MAC = "https://docs.flutter.dev/platform-integration/macos/building"
FL_ARCH = "https://docs.flutter.dev/resources/architectural-overview"
FL_PLAT = "https://docs.flutter.dev/reference/supported-platforms"
QT_DEPLOY = "https://doc.qt.io/qt-6/deployment.html"
QT_PERM = "https://doc.qt.io/qt-6/permissions.html"
QT_PLAT = "https://doc.qt.io/qt-6/supported-platforms.html"
AP_NOTARY = "https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution"

PAGES += [
    {
        "slug": "electron-alternatives.html",
        "title": "Electron alternatives in 2026, compared by what you ship",
        "description": "Electron alternatives in 2026: Tauri, Wails, Neutralinojs, Flutter, Qt and Krate, compared by what you hand to users and what they have to install.",
        "h1": "Electron alternatives in 2026, compared by what you ship",
        "lead": "The main Electron alternatives are Tauri, Wails, Neutralinojs, Flutter, Qt and Krate, and the biggest difference is what you hand to people: Krate ships one <code>.krate</code> file that runs natively on every desktop and opens from a link in any browser, while the others ship a build for each operating system. A Krate app starts with no file or network access until the person allows it.",
        "reviewed": REVIEWED,
        "entry_actions": [("See the table", "#the-short-list"), ("Krate vs Electron in detail", "/krate-vs-electron.html")],
        "sections": [
            ("The short list", comparison_table("Electron and its alternatives, from each project's official docs, read 6 October 2026", "Electron alternatives compared",
                ["Tool", "What you ship", "How it draws the UI", "App languages", "Phones"], [
                ["Electron", "A package per operating system, each with its own Chromium and Node.js.", "Its own Chromium.", "JavaScript, HTML, CSS.", "No."],
                ["Tauri", "A bundle or installer per operating system, plus store packages for mobile.", "The system WebView.", "A web frontend, with Rust in the core.", "Yes."],
                ["Wails", "A binary per operating system and architecture, with an optional Windows installer.", "The system WebView.", "Go, with a web frontend.", "Only in v3, marked experimental."],
                ["Neutralinojs", "A prebuilt binary per operating system and CPU, all made by one build on any host, plus a resources file.", "The system WebView.", "JavaScript, HTML, CSS.", "No."],
                ["Flutter", "A build per operating system (.exe with DLLs, .app, or a Linux package).", "Its own widgets and renderer, no WebView.", "Dart.", "Yes."],
                ["Qt", "A per-platform build, static or with Qt's shared libraries.", "Its own widgets or scene graph, no WebView by default.", "C++, QML, or Python through Qt for Python.", "Yes."],
                ["Krate", "One <code>.krate</code> file for every desktop, and a link that opens it in any browser.", "The Krate runtime draws a native window, with no browser or WebView.", "Describe it and Studio's AI writes it, or write Rust.", "Yes, from a link in the phone's browser."],
            ]) + f"""
<!-- Competitor rows read 6 October 2026 from the official docs linked below. -->
<p>Sources: <a href="{EL_HOME}">Electron</a> and <a href="{EL_FORGE}">Electron Forge</a>, <a href="{TA_DIST}">Tauri distribution</a> and <a href="{TA_WEBVIEW}">Tauri webviews</a>, <a href="{WA_INTRO}">Wails</a>, <a href="{NE_DOCS}">Neutralinojs</a> and its <a href="{NE_DIST}">distribution overview</a>, <a href="{FL_DESKTOP}">Flutter desktop</a> and <a href="{FL_PLAT}">supported platforms</a>, <a href="{QT_DEPLOY}">Qt deployment</a> and <a href="{QT_PLAT}">supported platforms</a>.</p>"""),
            ("What the user installs, and what the app can touch", comparison_table("Install and access, from each project's official docs, read 6 October 2026", "Install and access compared",
                ["Tool", "What the user installs", "What limits the app"], [
                ["Electron", "Your app's own package.", "Renderers run in Chromium's sandbox by default since Electron 20; the main process is not sandboxed."],
                ["Tauri", "Your app's own package; on Windows it relies on WebView2, which the installer can add on older systems.", "Capabilities limit what the frontend can call; the Rust core has full access to system resources."],
                ["Wails", "Your app's own binary; on Windows the WebView2 runtime is needed.", "The v2 guides linked here describe Go methods bound to the frontend, not a permission model."],
                ["Neutralinojs", "Your app's own binary and resources file.", "An allow list and a block list in the config control which native APIs the frontend can call."],
                ["Flutter", "Your app's own package; on Windows, the Visual C++ redistributables too.", "macOS builds use App Sandbox by default, with entitlements for network, camera or files."],
                ["Qt", "Your app, plus Qt's libraries unless the system already has them.", "Permission APIs ask for operating-system consent, such as the microphone; no Qt-level sandbox is described."],
                ["Krate", "Nothing, to open a link. The Krate runtime, once, to run apps natively; then every <code>.krate</code> opens from its file.", "The whole app starts with no file or network access; the person sees what it asks for and grants it before it opens."],
            ]) + f"""
<p>Sources: <a href="{EL_SANDBOX}">Electron sandbox</a>, <a href="{TA_SEC}">Tauri security</a> and <a href="{TA_CAPS}">capabilities</a>, <a href="{WA_WIN}">Wails on Windows</a>, <a href="{NE_CONF}">Neutralinojs configuration</a>, <a href="{FL_MAC}">Flutter on macOS</a>, <a href="{QT_PERM}">Qt permissions</a>.</p>"""),
            ("How to choose", """<ul>
<li><strong>You want one file for every screen, with permissions the user approves:</strong> Krate. It is the only option here where the same file runs natively on macOS, Windows and Linux and opens from a link in any browser.</li>
<li><strong>You have a web app and want it on the desktop as it is:</strong> Electron, or Tauri, Wails or Neutralinojs if you want the system WebView instead of a bundled Chromium.</li>
<li><strong>You want store apps for phones:</strong> Flutter, Qt or Tauri. For phones without a store, a Krate link opens in the phone's browser.</li>
<li><strong>You write C++ or need a long-established widget toolkit:</strong> Qt.</li>
</ul>
<p>Every native installer needs signing on macOS and Windows; the <a href="/desktop-app-code-signing.html">code signing page</a> explains what that takes, and the <a href="/how-to-distribute-a-desktop-app.html">distribution guide</a> walks through both routes.</p>"""),
            ("Measured against an Electron editor", MEASURED),
            ("Try Krate in five minutes", """<p><a href="/download/">Download Krate</a>, then open <a href="https://raw.githubusercontent.com/incyashraj/krate/c46d29500f2b894c89285b04e1b5e2f75184e972/evidence/ported/chart.krate">chart.krate</a>, an 11,455-byte chart app, on any of the three systems. The <a href="/docs/quickstart.html">quickstart</a> shows how to see what it asks for before it runs. Or describe your own app in <a href="/studio/">Krate Studio</a>.</p>"""),
        ],
        "faq": [
            ("Which Electron alternatives use the system WebView?", "<p>Tauri, Wails and Neutralinojs. Flutter and Qt draw their own interface, and Krate's runtime draws a native window with no browser or WebView.</p>"),
            ("Can I keep my web frontend?", "<p>Tauri, Wails and Neutralinojs keep it in a WebView. Krate brings the app over instead: <code>krate port</code> (beta) and Studio's AI rewrite it as one small native file with no browser inside.</p>"),
            ("Which alternative ships one file for every operating system?", "<p>Krate. Neutralinojs builds every platform's binary in one step, but you still ship one binary per system; a <code>.krate</code> is the same file on macOS, Windows and Linux, and a link opens it in any browser.</p>"),
            ("Do any of them sandbox apps by default?", "<p>Electron sandboxes renderers by default; Tauri limits what the frontend can call; Flutter's macOS builds use App Sandbox. In Krate the whole app starts with no file or network access until the person allows it.</p>"),
        ],
    },
    {
        "slug": "tauri-alternatives.html",
        "title": "Tauri alternatives in 2026, compared by what you ship",
        "description": "Tauri alternatives in 2026: Krate ships one .krate file for every desktop and a link for every browser; Electron, Wails and Flutter ship a build per OS.",
        "h1": "Tauri alternatives in 2026, compared by what you ship",
        "lead": "If you like Tauri's small apps but want to stop building, signing and hosting a bundle for each operating system, Krate ships one <code>.krate</code> file that runs natively on every desktop and opens from a link in any browser, phones included. Electron, Wails and Flutter are the other common choices, each with a build per OS.",
        "reviewed": REVIEWED,
        "entry_actions": [("See the table", "#side-by-side"), ("Krate vs Tauri in detail", "/krate-vs-tauri.html")],
        "sections": [
            ("Why people look past Tauri", f"""<p>Tauri is a good tool, and its apps are small because they use the system WebView. Three things send people looking anyway, all from Tauri's own docs:</p>
<ul>
<li><strong>A build per system.</strong> Tauri ships an app bundle and DMG on macOS, MSI or NSIS installers on Windows, and Linux packages, each built for that platform (<a href="{TA_DIST}">distribution</a>).</li>
<li><strong>A different WebView on each system.</strong> WebView2 on Windows, WKWebView on macOS and webkit2gtk on Linux, so the same frontend renders through three engines (<a href="{TA_WEBVIEW}">webview versions</a>).</li>
<li><strong>Permissions cover the frontend, not the core.</strong> Capabilities limit what the WebView can call; code in the Rust core and plugins has full access to system resources (<a href="{TA_SEC}">security</a>).</li>
</ul>"""),
            ("Side by side", comparison_table("Tauri and its alternatives, from each project's official docs, read 6 October 2026", "Tauri alternatives compared",
                ["Tool", "What you ship", "Rendering", "What limits the app", "Phones"], [
                ["Tauri", "A bundle or installer per operating system.", "The system WebView.", "Capabilities for the frontend; the Rust core is unconstrained.", "Yes."],
                ["Krate", "One <code>.krate</code> file for every desktop, and a link for any browser.", "The Krate runtime draws a native window; no WebView.", "The whole app starts with no file or network access until the person allows it.", "Yes, from a link in the phone's browser."],
                ["Electron", "A package per operating system with its own Chromium and Node.js.", "Bundled Chromium.", "Renderers sandboxed by default; the main process is not.", "No."],
                ["Wails", "A binary per operating system and architecture.", "The system WebView.", "Go methods bound to the frontend.", "v3 only, experimental."],
                ["Flutter", "A build per operating system.", "Its own renderer; no WebView.", "App Sandbox by default on macOS builds.", "Yes."],
            ]) + f"""
<!-- Competitor rows read 6 October 2026 from the official docs linked below. -->
<p>Sources: <a href="{TA_START}">Tauri</a>, <a href="{EL_SANDBOX}">Electron sandbox</a>, <a href="{WA_XPLAT}">Wails cross-platform builds</a>, <a href="{FL_ARCH}">Flutter architecture</a> and <a href="{FL_MAC}">Flutter on macOS</a>.</p>"""),
            ("Krate for Tauri developers", """<p>Both are Rust. What changes is where the app runs and what you hand people. A Krate app is a WebAssembly component that calls Krate's interfaces for its window, drawing, files, network and storage. The Krate runtime runs the same file natively on every desktop and checks protected calls against what the person granted, and a link opens the same app in any browser.</p>
<p>There is no WebView, so the interface is written against Krate's interfaces, by hand or by Krate Studio's AI. Rust logic that builds for WebAssembly comes with you. Start with a scan that reads your project without building or running it:</p>
<pre class="answer-cmd">krate port ./my-tauri-app</pre>
<p>It lists what maps cleanly and what needs a permission. <code>krate port</code> is in beta, and the <a href="/docs/porting.html">porting guide</a> covers the rest.</p>"""),
        ],
        "faq": [
            ("Can I reuse my Tauri Rust code in Krate?", "<p>Yes, logic that builds for WebAssembly usually carries over. Calls into the operating system and the WebView interface move to Krate's interfaces, and <code>krate port</code> (beta) reports what each part takes.</p>"),
            ("Does Krate use a WebView?", "<p>No. The Krate runtime draws the app's window itself, so your app does not render through three different browser engines.</p>"),
            ("Do the Tauri alternatives support phones?", "<p>Flutter does, and Wails v3 has experimental mobile support; Electron does not. Krate opens the same file from a link in any phone browser, with nothing to install.</p>"),
            ("What do people install to open a Krate app?", "<p>Nothing, to open a link. To run apps natively, the Krate runtime, once for their system; after that every <code>.krate</code> opens from the file and shows what it asks for first.</p>"),
        ],
    },
    {
        "slug": "cross-platform-desktop-app-frameworks.html",
        "title": "Cross-platform desktop app frameworks in 2026 | Krate",
        "description": "Cross-platform desktop app frameworks in 2026, compared by what you ship: Electron, Tauri, Flutter, Qt and Krate, which ships one file for every screen.",
        "h1": "Cross-platform desktop app frameworks in 2026, compared by what you ship",
        "lead": "Krate is the framework that ships one file for every screen: the same <code>.krate</code> runs natively on Mac, Windows and Linux and opens from a link in any browser, phones included. Electron, Tauri, Flutter and Qt each ship a build per operating system, and the table below shows how they compare.",
        "reviewed": REVIEWED,
        "entry_actions": [("See the comparison", "#the-comparison"), ("How to distribute a desktop app", "/how-to-distribute-a-desktop-app.html")],
        "sections": [
            ("The comparison", comparison_table("Five frameworks, from each project's official docs, read 6 October 2026", "Desktop frameworks compared",
                ["Question", "Electron", "Tauri", "Flutter", "Qt", "Krate"], [
                ["What you ship", "A package per OS", "A bundle per OS", "A build per OS", "A build per platform", "One <code>.krate</code> file, and a link"],
                ["What the user installs", "Your package", "Your package; WebView2 on older Windows", "Your package; VC++ redistributables on Windows", "Your app with Qt's libraries", "Nothing for a link; the Krate runtime, once, to run natively"],
                ["Per-OS builds", "Yes; some installers only build on their own OS", "Yes; macOS bundles on a Mac, MSI on Windows", "Yes, with each OS's toolchain", "Yes, each platform's environment", "No; one build"],
                ["Signing", "Sign and notarize per OS", "Sign and notarize per OS", "Notarize on macOS; signed MSIX on Windows", "macdeployqt can sign for notarization", "None per OS for the app file"],
                ["Sandbox and permissions", "Renderers sandboxed; main process not", "Capabilities for the frontend; core unconstrained", "App Sandbox on macOS builds", "OS consent APIs; no Qt sandbox", "Whole app starts with no file or network access"],
                ["App languages", "JavaScript, HTML, CSS", "Web frontend, Rust core", "Dart", "C++, QML, Python", "Rust, or described in plain words to Studio's AI"],
                ["Phones", "No", "Yes", "Yes", "Yes", "Yes, from a link in the browser"],
                ["Current release", "Latest three majors supported", "Stable v2; v3 in alpha", "Stable, 3.47", "Qt 6.12, September 2026", "0.5.4, September 2026"],
            ]) + f"""
<!-- Rows for Electron, Tauri, Flutter and Qt read 6 October 2026 from the official docs linked below. -->
<p>Sources: <a href="{EL_DIST}">Electron distribution</a>, <a href="{EL_SIGN}">signing</a> and <a href="{EL_SANDBOX}">sandbox</a>; <a href="{TA_DIST}">Tauri distribution</a>, <a href="{TA_SEC}">security</a> and <a href="{TA_WEBVIEW}">webviews</a>; <a href="{FL_DESKTOP}">Flutter desktop</a>, <a href="{FL_MAC}">macOS builds</a> and <a href="{FL_PLAT}">platforms</a>; <a href="{QT_DEPLOY}">Qt deployment</a>, <a href="{QT_PERM}">permissions</a> and <a href="{QT_PLAT}">platforms</a>. Wails and Neutralinojs are on the <a href="/electron-alternatives.html">Electron alternatives</a> page.</p>"""),
            ("Three questions that decide it", """<ol>
<li><strong>Do you want to hand people one file for every screen?</strong> Then Krate. Every other option here produces a separate build for each operating system.</li>
<li><strong>Do you need phones?</strong> Flutter, Qt and Tauri build store apps. A Krate app opens on a phone from a link, with no store and nothing to install.</li>
<li><strong>Do you have a web codebase you want to keep as it is?</strong> Then Electron or Tauri. Flutter, Qt and Krate each draw their own interface, and Krate Studio's AI can rewrite yours.</li>
</ol>
<p>A fourth question sits under all of them: who decides what the app may touch? In most frameworks the developer decides. In Krate the person running the app sees what it asks for and grants it, and the runtime enforces that.</p>"""),
            ("Signing and installers, whichever you pick", f"""<p>A native installer is signed for each system. On macOS, software distributed with a Developer ID must be notarized by Apple (<a href="{AP_NOTARY}">Apple</a>). On Windows, unsigned apps show a SmartScreen warning, and new signed apps can still warn until they build reputation (<a href="{MS_SS}">Microsoft</a>).</p>
<p>A <code>.krate</code> file is not a native installer, so there is no per-OS signing of the app, and a link needs no install at all. On macOS the Krate runtime is signed and notarized by Apple. More on the <a href="/desktop-app-code-signing.html">code signing page</a>.</p>"""),
            ("One file, proven on three systems", f"""<p>The same <code>.krate</code> files pass automated checks on macOS, Windows and Linux in CI: 10 apps on all three at Krate 0.5.4.</p>
{THREE_OS_REPLAY}"""),
            ("Measured against an Electron editor", MEASURED),
        ],
        "faq": [
            ("What is the best cross-platform desktop app framework?", "<p>For one file on every screen, Krate: it runs natively on every desktop and opens from a link in any browser. For an existing web codebase, Electron or Tauri; for store apps on phones, Flutter, Qt or Tauri.</p>"),
            ("Which framework needs no build per operating system?", "<p>Krate: you build one <code>.krate</code> file and the runtime on each system opens it. The others produce a build for each operating system.</p>"),
            ("Which frameworks avoid a browser engine?", "<p>Flutter and Qt draw their own interface, and so does Krate's runtime. Electron bundles Chromium, and Tauri uses the system WebView.</p>"),
            ("Is Krate open source?", '<p>Yes. The runtime, the <code>.krate</code> format and the CLI are MIT OR Apache-2.0, and Krate Studio is under the Business Source License 1.1. <a href="https://github.com/incyashraj/krate">The source is on GitHub</a>.</p>'),
        ],
    },
]


class MetadataTests(unittest.TestCase):
    def test_head_is_built_per_page_never_borrowed(self):
        # The head used to be lifted from the homepage and its identity
        # scrubbed out; when the homepage was rebuilt that scraping broke
        # silently. Each head is now written from the page's own data.
        for page in PAGES:
            result = page_head(page)
            for other in PAGES:
                if other is not page:
                    self.assertNotIn(html.escape(other["title"], quote=True), result)
            self.assertNotIn('content="noindex"', result)
            self.assertIn(K.KIT_LINKS, result)
            self.assertIn(K.HEAD_THEME, result)

    def test_each_page_owns_its_metadata(self):
        for page in PAGES:
            with self.subTest(slug=page["slug"]):
                result = render(page)
                self.assertEqual(result.count("<title>"), 1)
                self.assertEqual(result.count('rel="canonical"'), 1)
                self.assertEqual(result.count('property="og:url"'), 1)
                self.assertEqual(result.count('name="description"'), 1)
                self.assertEqual(result.count('name="twitter:title"'), 1)
                schemas = re.findall(r'<script type="application/ld\+json">(.*?)</script>', result, re.S)
                self.assertEqual(len(schemas), 1)
                schema = json.loads(schemas[0])
                self.assertEqual(schema["@type"], "FAQPage" if page.get("faq") else "WebPage")
                self.assertEqual(schema["publisher"]["@id"], "https://krate.tech/#organization")
                self.assertEqual(schema["about"]["@id"], "https://krate.tech/#runtime")
                self.assertEqual(schema["name"], page["title"])
                self.assertEqual(schema["url"], "https://krate.tech/" + page["slug"])
                self.assertIn("/docs/quickstart.html", result)
                self.assertEqual(result.count("<h1>"), 1)

    def test_unique_page_intents(self):
        for key in ("slug", "title", "description", "h1"):
            self.assertEqual(len({p[key] for p in PAGES}), len(PAGES), key)

    def test_entry_actions_precede_article_and_keep_maintained_paths(self):
        for page in PAGES:
            with self.subTest(slug=page["slug"]):
                result = render(page)
                entry = result.index('<nav class="acts answer-actions" aria-label="Choose your next step">')
                self.assertLess(entry, result.index('<section id="'))
                self.assertEqual(len(page["entry_actions"]), 2)
                for label, href in page["entry_actions"]:
                    self.assertIn(html.escape(label), result)
                    self.assertIn(f'href="{html.escape(href, quote=True)}"', result)
                for href in ("/docs/quickstart.html", "/docs/porting.html", "/docs/limits.html", "/studio/"):
                    self.assertIn(f'href="{href}"', result)
                self.assertIn(K.HEADER, result)
                self.assertIn(K.FOOTER, result)
                self.assertNotIn('href="/#install"', result)

    def test_all_same_page_actions_have_unique_targets(self):
        for page in PAGES:
            result = render(page)
            ids = re.findall(r'\bid="([^"]+)"', result)
            self.assertEqual(len(ids), len(set(ids)), page["slug"])
            for fragment in re.findall(r'href="#([^"]+)"', result):
                self.assertIn(fragment, ids, f"{page['slug']}#{fragment}")

    def test_sharing_routes_existing_apps_before_authoring_setup(self):
        page = next(p for p in PAGES if p["slug"] == "share-an-app-made-with-ai.html")
        result = render(page)
        self.assertLess(result.index("Already have a"), result.index("Make it by describing it"))
        self.assertIn('href="#send-it-as-a-link-or-a-file"', result)
        self.assertIn('href="#test-before-you-send-it"', result)
        self.assertIn('href="/open/"', result)
        self.assertIn('href="/docs/porting.html"', result)
        self.assertIn("https://hub.krate.tech/run/", result)
        self.assertIn("Publishing needs a Krate account; opening never does", result)

    def test_portability_proof_links_the_tested_file_and_per_host_evidence(self):
        page = next(p for p in PAGES if p["slug"] == "portable-desktop-app-format.html")
        result = render(page)
        bundle_ref = "c46d29500f2b894c89285b04e1b5e2f75184e972"
        ci_ref = "7701ad231e841c219f8493ad402b3ea0ff503af4"
        self.assertIn(f'https://raw.githubusercontent.com/incyashraj/krate/{bundle_ref}/evidence/ported/chart.krate', result)
        self.assertIn(f'https://github.com/incyashraj/krate/tree/{bundle_ref}/apps/krate-chart', result)
        self.assertIn("3d290c48f74936d5cdb45ceb0ee945bd0f7b5b0b21f87f79917bced3b4ca73c3", result)
        self.assertIn('href="/docs/quickstart.html#try-the-chart-sample"', result)
        self.assertIn('href="https://github.com/incyashraj/krate/actions/runs/35454102875"', result)
        for host in ("macos-latest", "windows-2022", "ubuntu-latest"):
            for suffix in ("chart.png", "chart.png.json"):
                self.assertIn(f'https://github.com/incyashraj/krate/blob/{ci_ref}/evidence/golden/{host}/{suffix}', result)
        for scope in ("headless replay", "source-built runtimes",
                      "is available separately", "its own reference image"):
            self.assertIn(scope, result)

    def test_duplicate_section_anchors_fail_before_generation(self):
        page = dict(PAGES[0], sections=[("Same title", "<p>One</p>"), ("Same title!", "<p>Two</p>")])
        with self.assertRaisesRegex(ValueError, "Section anchors must be unique"):
            render(page)

    def test_distribution_comparison_proves_the_shipping_difference(self):
        page = next(p for p in PAGES if p["slug"] == "desktop-app-distribution.html")
        result = render(page)
        self.assertIn("Krate vs Electron vs Tauri", page["title"])
        self.assertEqual(result.count('class="shipping-lane'), 2)
        self.assertIn("typical desktop packaging", result.lower())
        self.assertIn("app.asar", result)
        self.assertIn("not a standalone cross-OS executable", result)
        self.assertIn('href="https://www.electronjs.org/docs/latest/tutorial/application-distribution"', result)
        self.assertIn('href="https://tauri.app/distribute/"', result)
        self.assertIn('href="/portable-desktop-app-format.html#try-one-file-tested-on-all-three-systems"', result)
        self.assertIn("installed once and shared by every Krate app", result)
        self.assertIn("opens from a link in any browser", result)
        self.assertIn('href="/reports/"', result)

    def test_measured_numbers_carry_their_date_machine_and_method(self):
        pages = [p for p in PAGES if any(body == MEASURED for _, body in p["sections"])]
        self.assertGreaterEqual(len(pages), 5)
        for figure in ("36.6 KiB", "178.5 MiB", "2,299.4 MiB", "237.1 ms", "611.5 ms", "284.6 MiB", "13x less", "2.6x faster"):
            self.assertIn(figure, MEASURED)
        for scope in ("25 August 2026", "Apple M4", "MarkText 0.17.1", "50,000-line notes workload",
                      "an equivalent workload", 'href="/reports/"'):
            self.assertIn(scope, MEASURED)
        for page in PAGES:
            if "13x" in page["lead"]:
                self.assertIn("25 August 2026", page["lead"], page["slug"])
                self.assertIn("Apple M4", page["lead"], page["slug"])

    def test_every_page_says_a_link_opens_anywhere_and_retired_lines_stay_gone(self):
        retired = ("Phones are not supported", "desktop only today", "Desktop only today", "Not yet.",
                   "guarantee", "cannot do anything", "15-40 KB", "80-200 MB", "behaves identically",
                   "does not need to trust you", "\u2014")
        for page in PAGES:
            with self.subTest(slug=page["slug"]):
                result = render(page)
                self.assertRegex(page["lead"], r"link|browser", "the lead must say a link opens it")
                for phrase in retired:
                    self.assertNotIn(phrase, result)

    def test_github_discovery_action_remains_visible_without_script(self):
        for page in PAGES:
            result = render(page)
            self.assertIn('href="https://github.com/incyashraj/krate">Explore Krate on GitHub</a>', result)

    def test_article_shell_is_the_kit_and_commands_never_widen_the_page(self):
        # The shell is the design kit's; the page adds only what its own
        # content needs. A command block wraps inside its own box (at 390px
        # one long line once made every answer page scroll sideways, and a
        # scrolling box hid the end of a long command even at 1440).
        self.assertIn(".prose pre.answer-cmd", ANSWER_CSS)
        self.assertIn("overflow-x: auto", ANSWER_CSS)
        self.assertIn("white-space: pre-wrap;", ANSWER_CSS)
        self.assertIn("max-width: 100%", ANSWER_CSS)
        self.assertIn("overflow-wrap: anywhere", ANSWER_CSS)
        for page in PAGES:
            result = render(page)
            self.assertIn(ANSWER_CSS, result)
            self.assertIn('<nav class="toc" aria-label="On this page">', result)
            for t, _ in page["sections"]:
                self.assertIn(f'<a href="#{section_id(t)}">', result)
            for opening in re.findall(r'<pre\b[^>]*>', result):
                self.assertIn('tabindex="0"', opening)
                self.assertIn('aria-label="Command example"', opening)

    def test_scroll_revealed_selectors_are_readable_without_javascript(self):
        # Anything that starts hidden and waits for a script -- a reveal on
        # scroll, a rise, a bar that grows -- is invisible to a reader with
        # JavaScript off. Since the 2026-10 redesign every page, the homepage
        # included, takes its reveals from the design kit, where a hidden
        # start is allowed only under `.js` (set by the head script). This
        # reads the kit's stylesheet; the homepage's own illustrations are
        # checked in a browser with scripts off by scripts/test-site-nojs.mjs.
        kit = (ROOT / "docs/landing/kit/site.css").read_text()
        self.assertEqual(K.hidden_ungated(kit), [], "a kit rule hides something without .js")
        self.assertIn(".js .rv { opacity: 0;", kit, "the reveal is gated on .js")
        home = LANDING.read_text()
        self.assertNotRegex(home, r"(?m)^\.rv \{", "the homepage must not define its own ungated reveal")
        self.assertIn("d.classList.add('js')", home, "the homepage marks .js before first paint")

    def test_questions_are_visible_and_match_the_structured_data(self):
        # Structured data must describe what a reader can see on the page.
        faq_pages = [p for p in PAGES if p.get("faq")]
        self.assertGreaterEqual(len(faq_pages), 6)
        for page in faq_pages:
            with self.subTest(slug=page["slug"]):
                result = render(page)
                schema = json.loads(re.findall(r'<script type="application/ld\+json">(.*?)</script>', result, re.S)[0])
                self.assertEqual(len(schema["mainEntity"]), len(page["faq"]))
                for entity, (question, answer) in zip(schema["mainEntity"], page["faq"]):
                    self.assertEqual(entity["name"], question)
                    self.assertIn(f"<h3>{html.escape(question)}</h3>", result)
                    text = entity["acceptedAnswer"]["text"]
                    self.assertNotIn("<", text)
                    self.assertTrue(text and text[0].isupper(), question)
                self.assertLess(result.index('<section id="questions">'), result.index('<section id="more-answers">'))

    def test_reviewed_pages_carry_a_visible_date(self):
        for page in PAGES:
            if not page.get("reviewed"):
                continue
            with self.subTest(slug=page["slug"]):
                result = render(page)
                schema = json.loads(re.findall(r'<script type="application/ld\+json">(.*?)</script>', result, re.S)[0])
                self.assertEqual(schema["dateModified"], page["reviewed"])
                self.assertIn(f'<time datetime="{page["reviewed"]}">', result)
                self.assertIn(f"against Krate {REVIEWED_VERSION}.", result)

    def test_every_page_links_every_other_answer(self):
        for page in PAGES:
            result = render(page)
            start = result.index('<section id="more-answers">')
            related = result[start:result.index('</section>', start)]
            for other in PAGES:
                if other is not page:
                    self.assertIn(f'href="/{other["slug"]}"', related, page["slug"])
            self.assertNotIn(f'href="/{page["slug"]}"', related)

    def test_internal_answer_links_resolve(self):
        slugs = {p["slug"] for p in PAGES}
        for page in PAGES:
            for target in re.findall(r'href="/([a-z0-9-]+\.html)', render(page)):
                self.assertIn(target, slugs, f"{page['slug']} links /{target}")

    def test_shipping_comparisons_cite_official_sources(self):
        expected = {
            "krate-vs-electron.html": ("https://www.electronjs.org/docs/latest/tutorial/code-signing",
                                       "https://www.electronjs.org/docs/latest/tutorial/sandbox", "Moving an Electron app to Krate"),
            "krate-vs-tauri.html": ("https://tauri.app/distribute/", "https://tauri.app/security/capabilities/",
                                    "Moving a Tauri app to Krate"),
            "desktop-app-code-signing.html": ("https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/smartscreen-reputation",
                                              "https://www.electronjs.org/docs/latest/tutorial/code-signing", "What you get back"),
        }
        for slug, (first, second, heading) in expected.items():
            result = render(next(p for p in PAGES if p["slug"] == slug))
            self.assertIn(f'href="{first}"', result)
            self.assertIn(f'href="{second}"', result)
            self.assertIn(f"<h2>{heading}</h2>", result)

    def test_distribution_page_links_the_three_os_replay_records(self):
        result = render(next(p for p in PAGES if p["slug"] == "how-to-distribute-a-desktop-app.html"))
        for host in ("macos", "ubuntu", "windows"):
            path = f"evidence/e3/run-36557127592-replay-{host}.tsv"
            self.assertTrue((ROOT / path).is_file(), path)
            self.assertIn(f"/blob/{PIN}/{path}", result)
        self.assertIn("3d290c48f74936d5cdb45ceb0ee945bd0f7b5b0b21f87f79917bced3b4ca73c3", result)
        self.assertIn("headless replay checks", result)

    def test_llms_full_carries_the_short_file_and_every_page(self):
        text = render_llms_full()
        self.assertTrue(text.startswith(load_public_facts().render_llms().rstrip()))
        for page in PAGES:
            self.assertIn(f"URL: https://krate.tech/{page['slug']}", text)
            self.assertIn(f"## {page['h1']}", text)
            for question, _ in page.get("faq", []):
                self.assertIn(question, text)
        self.assertNotIn("<a ", text)
        self.assertNotIn("<code>", text)

    def test_committed_llms_full_carries_the_current_answers(self):
        # The answers half is this file's output; the llms.txt half is
        # checked by public_facts.py --check, so compare from the heading.
        committed = LLMS_FULL.read_text()
        self.assertTrue(committed.startswith("# Krate\n"))
        expected = render_llms_full()
        self.assertEqual(committed[committed.index(ANSWERS_HEADING):],
                         expected[expected.index(ANSWERS_HEADING):],
                         "stale docs/landing/llms-full.txt: run scripts/build-answer-pages.py")

    def test_answer_pages_never_redirect_to_studio(self):
        # A signed-in reader arriving from a search result must see the answer.
        for page in PAGES:
            result = render(page)
            self.assertNotIn('location.replace("/app/")', result, page["slug"])
            self.assertIn(K.HEAD_THEME, result)
        self.assertIn('location.replace("/app/")', LANDING.read_text())

    def test_answers_index_lists_every_page_and_is_current(self):
        result = render_index()
        for page in PAGES:
            self.assertIn(f'href="/{page["slug"]}"', result)
            self.assertIn(html.escape(first_sentence(page["lead"])), result)
        self.assertIn('<link rel="canonical" href="https://krate.tech/answers/">', result)
        self.assertEqual(INDEX.read_text(), result, "stale docs/landing/answers/index.html: run scripts/build-answer-pages.py")

    def test_metadata_is_escaped(self):
        page = dict(PAGES[0], title='A "quoted" <title> & more', description='Keep </script> as text')
        result = page_head(page)
        self.assertIn("&quot;quoted&quot; &lt;title&gt; &amp; more", result)
        schemas = re.findall(r'<script type="application/ld\+json">(.*?)</script>', result, re.S)
        self.assertEqual(json.loads(schemas[0])["description"], page["description"])


# The answers index: served at /answers/ from docs/landing, never from
# docs/answers (the deploy copies docs/answers/*.html to the site root, so an
# index.html there would replace the homepage).
INDEX = ROOT / "docs" / "landing" / "answers" / "index.html"


def first_sentence(fragment):
    text = plain_text(fragment)
    cut = re.search(r"(?<=\.)\s", text)
    return text[:cut.start()] if cut else text


def render_index():
    title = "Shipping desktop apps: answers | Krate"
    description = ("Straight answers about making an app with Krate and sending it anywhere: one file for every "
                   "desktop, a link for every browser, Electron and Tauri, code signing, and apps built with AI.")
    url = "https://krate.tech/answers/"
    schema = json.dumps({
        "@context": "https://schema.org", "@type": "CollectionPage", "@id": url + "#webpage", "url": url,
        "name": title, "description": description, "inLanguage": "en",
        "isPartOf": {"@type": "WebSite", "@id": "https://krate.tech/#website", "name": "Krate", "url": "https://krate.tech/"},
        "publisher": PUBLISHER, "about": RUNTIME,
        "hasPart": [{"@type": "WebPage", "url": "https://krate.tech/" + p["slug"], "name": p["h1"]} for p in PAGES],
    }, ensure_ascii=False).replace("<", "\\u003c")
    items = "\n".join(f'''    <a class="card link ans-item" href="/{p["slug"]}"><b>{html.escape(p["h1"])}</b><span>{html.escape(first_sentence(p["lead"]))}</span></a>''' for p in PAGES)
    return f'''<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover">
<title>{title}</title>
<meta name="description" content="{html.escape(description, quote=True)}">
<link rel="canonical" href="{url}">
<meta property="og:type" content="website">
<meta property="og:site_name" content="Krate">
<meta property="og:title" content="{title}">
<meta property="og:description" content="{html.escape(description, quote=True)}">
<meta property="og:url" content="{url}">
<meta property="og:image" content="https://krate.tech/og-v5.png">
<meta name="twitter:card" content="summary_large_image">
<link rel="icon" href="/krate-favicon.png">
<meta name="theme-color" content="#fbfbfd" media="(prefers-color-scheme: light)">
<meta name="theme-color" content="#08080a" media="(prefers-color-scheme: dark)">
<script type="application/ld+json">{schema}</script>
{K.HEAD_THEME}
{K.KIT_LINKS}
<style>
.ans-list {{ display: grid; gap: 12px; margin-top: 8px; }}
.ans-item {{ display: grid; gap: 6px; padding: 22px 24px; }}
.ans-item b {{ font-size: 17px; font-weight: 600; letter-spacing: -.015em; color: var(--ink); }}
.ans-item span {{ font-size: 14.5px; line-height: 1.6; color: var(--mute); }}
.ph .lede a {{ color: var(--accent); }}
</style>
</head>
<body>
{K.HEADER}
{K.MNAV}

<main id="main">
<section class="ph wrap n">
  <span class="eye">Answers</span>
  <h1>Shipping desktop apps: answers</h1>
  <p class="lede">Straight answers about making an app with Krate and getting it to people: one small .krate file that opens from a link in any browser, phones included, and runs natively on Mac, Windows and Linux. The <a href="/facts/">facts page</a> has the short version of what Krate is.</p>
</section>
<section class="sec t wrap n">
  <div class="ans-list">
{items}
  </div>
</section>
</main>

{K.FOOTER}
{K.KIT_SCRIPT}
</body>
</html>
'''


def load_public_facts():
    spec = importlib.util.spec_from_file_location("public_facts", ROOT / "scripts" / "public_facts.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def render_llms_full():
    """llms.txt, then the plain text of every answer page, for AI readers."""
    parts = [load_public_facts().render_llms().rstrip(), "",
             ANSWERS_HEADING, ""]
    for page in PAGES:
        parts += [f"## {page['h1']}", "", f"URL: https://krate.tech/{page['slug']}"]
        if page.get("reviewed"):
            parts.append(f"Reviewed: {page['reviewed']} against Krate {REVIEWED_VERSION}")
        parts += ["", plain_text(page["lead"], links=True), ""]
        for title, body in page["sections"]:
            parts += [f"### {title}", "", plain_text(body, links=True), ""]
        if page.get("faq"):
            parts += ["### Questions", ""]
            for question, answer in page["faq"]:
                parts += [f"Q: {question}", f"A: {plain_text(answer, links=True)}", ""]
    return "\n".join(parts).rstrip() + "\n"


def main() -> int:
    if "--llms-full" in sys.argv:
        index = sys.argv.index("--llms-full")
        if index + 1 >= len(sys.argv):
            raise SystemExit("usage: build-answer-pages.py --llms-full PATH")
        target = Path(sys.argv[index + 1])
        target.write_text(render_llms_full())
        print(f"  wrote {target}")
        return 0
    OUT.mkdir(parents=True, exist_ok=True)
    for page in PAGES:
        target = OUT / page["slug"]
        target.write_text(render(page))
        print(f"  wrote docs/answers/{page['slug']}")
    LLMS_FULL.write_text(render_llms_full())
    print("  wrote docs/landing/llms-full.txt")
    INDEX.parent.mkdir(parents=True, exist_ok=True)
    INDEX.write_text(render_index())
    print("  wrote docs/landing/answers/index.html")
    return 0


if __name__ == "__main__":
    if "--self-test" in sys.argv:
        unittest.main(argv=[sys.argv[0]])
    else:
        raise SystemExit(main())
