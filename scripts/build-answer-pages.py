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
      <h2>More answers about shipping desktop apps</h2>
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
<meta property="og:image" content="https://krate.tech/og-v4.png">
<meta property="og:image:alt" content="Krate desktop application runtime">
<meta name="twitter:card" content="summary_large_image">
<meta name="twitter:title" content="{title}">
<meta name="twitter:description" content="{description}">
<meta name="twitter:image" content="https://krate.tech/og-v4.png">
<link rel="icon" href="/krate-favicon.png">
<meta name="theme-color" content="#fbfbfd" media="(prefers-color-scheme: light)">
<meta name="theme-color" content="#08080a" media="(prefers-color-scheme: dark)">
<script type="application/ld+json">{schema}</script>
{K.HEAD_THEME}
{K.KIT_LINKS}
{ANSWER_CSS}
</head>'''


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
  <h1>{html.escape(page["h1"])}</h1>
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
  <div><h2>Build with Krate</h2><p>Start with the runtime and a project that fits the current APIs. The runtime and CLI are MIT OR Apache-2.0; Studio has a separate license. Check the <a href="https://github.com/incyashraj/krate#license">licensing details</a> and <a href="/docs/limits.html">capability limits</a>.</p></div>
  <div class="acts">
    <a class="btn dark" href="/docs/quickstart.html">Developer quickstart {ARROW}</a>
    <a class="btn ghost" href="/docs/porting.html">Evaluate your app</a>
    <a class="btn ghost" href="/studio/">Make an app in Studio</a>
    <a class="btn link" href="https://github.com/incyashraj/krate">Explore Krate on GitHub</a>
  </div>
</div>
</main>

{K.FOOTER}
{K.KIT_SCRIPT}
</body>
</html>
"""


PAGES = [
    {
        "slug": "portable-desktop-app-format.html",
        "title": "One desktop app file for macOS, Windows and Linux | Krate",
        "description": "How the .krate format separates your application from native runtimes: bundle contents, recipient requirements, portability checks and current limits.",
        "h1": "One desktop app file, three operating systems",
        "lead": "Build against Krate's interfaces and distribute one .krate artifact. Each recipient runs it through a compatible native Krate runtime on macOS, Windows or Linux.",
        "entry_actions": [("Try the same-file workflow", "/docs/quickstart.html"),
                          ("Check your project's fit", "/docs/porting.html")],
        "sections": [
            ("What is portable, and what is installed?", """<p>The <strong>application file</strong> stays the same. The <strong>runtime</strong> is installed for each machine's operating system and CPU. It executes the WebAssembly component and handles permitted host operations. Your users do not need your development toolchain to open a packed app.</p>
<p>This is not a converter for arbitrary Windows executables, native libraries or existing Electron/Tauri packages. The application must use Krate's supported interfaces and a compatible format/API version. See the <a href="/desktop-app-distribution.html">distribution-model comparison</a>.</p>"""),
            ("Try one file tested on all three systems", """<p>Download <a href="https://raw.githubusercontent.com/incyashraj/krate/c46d29500f2b894c89285b04e1b5e2f75184e972/evidence/ported/chart.krate">chart.krate</a>, a bar chart of a week's rainfall. Follow the <a href="/docs/quickstart.html#try-the-chart-sample">tested sample instructions</a> to inspect its permissions and run it. The <a href="https://github.com/incyashraj/krate/tree/c46d29500f2b894c89285b04e1b5e2f75184e972/apps/krate-chart">Rust source</a> is available separately.</p>
<p>Full-file SHA-256: <code>3d290c48f74936d5cdb45ceb0ee945bd0f7b5b0b21f87f79917bced3b4ca73c3</code>.</p>
<p>The same bundle passed automated headless replay and rendering checks on macOS, Windows and Linux in <a href="https://github.com/incyashraj/krate/actions/runs/35454102875">this CI run</a> with source-built runtimes. Each host checks its own reference image; this is CI evidence, not an interactive test.</p>
<ul>
<li>macOS: <a href="https://github.com/incyashraj/krate/blob/7701ad231e841c219f8493ad402b3ea0ff503af4/evidence/golden/macos-latest/chart.png">reference image</a> and <a href="https://github.com/incyashraj/krate/blob/7701ad231e841c219f8493ad402b3ea0ff503af4/evidence/golden/macos-latest/chart.png.json">recorded bundle hash</a>.</li>
<li>Windows: <a href="https://github.com/incyashraj/krate/blob/7701ad231e841c219f8493ad402b3ea0ff503af4/evidence/golden/windows-2022/chart.png">reference image</a> and <a href="https://github.com/incyashraj/krate/blob/7701ad231e841c219f8493ad402b3ea0ff503af4/evidence/golden/windows-2022/chart.png.json">recorded bundle hash</a>.</li>
<li>Linux: <a href="https://github.com/incyashraj/krate/blob/7701ad231e841c219f8493ad402b3ea0ff503af4/evidence/golden/ubuntu-latest/chart.png">reference image</a> and <a href="https://github.com/incyashraj/krate/blob/7701ad231e841c219f8493ad402b3ea0ff503af4/evidence/golden/ubuntu-latest/chart.png.json">recorded bundle hash</a>.</li>
</ul>"""),
            ("What is inside a .krate file?", """<p>The bundle is a ZIP-based application format, not a renamed executable. Its core entries are <code>manifest.toml</code> and <code>code.wasm</code>. Bundles can also contain assets, source, SDK material and format-specific metadata. The normal authoring workflow carries editable source with the app.</p>
<p>The manifest identifies the app and its requested capabilities. The component calls Krate interfaces described in WIT; the native runtime supplies their implementations. Files, network and other host resources are governed by the capability model.</p>
<p>Read the <a href="https://github.com/incyashraj/krate/tree/main/crates/bundle">bundle implementation</a> and <a href="https://github.com/incyashraj/krate/tree/main/wit">interface definitions</a>. Do not put credentials or private inputs in source or assets that will be distributed.</p>"""),
            ("Verify a shared artifact yourself", """<p>Start with an app you built or reviewed. Install a compatible runtime on each target machine, copy the <em>same</em> file, record <code>krate --version</code>, inspect permissions and run it:</p>
<pre class="answer-cmd">krate --version
krate run app.krate --dump-caps
krate run app.krate --prompt</pre>
<p>Compare the full-file SHA-256 on each host. On macOS use <code>shasum -a 256 app.krate</code>; on Linux use <code>sha256sum app.krate</code>; in Windows PowerShell use <code>Get-FileHash app.krate -Algorithm SHA256</code>. Matching hashes establish identical bytes, not correctness or trust.</p>
<p>Then complete the same meaningful task on each OS: load representative data, interact with the UI, save, close and reopen. Test denied permissions and OS-specific file dialogs. Keep runtime versions, hashes and results together. This is a reproduction procedure, not a claim that every application has already passed that test.</p>"""),
            ("Does one file remove all platform work?", """<p>No. Krate maintains native runtime builds. Developers still need to test behavior on target systems, check supported capabilities and manage API/format compatibility. A runtime update may be necessary for apps using newer interfaces.</p>
<p>The benefit is a shared application artifact rather than a separate app package for each OS. OS installation, trust checks and updates still apply to the runtime. The format does not make unsupported APIs available or bypass platform security.</p>"""),
            ("What about size and performance?", """<p>Measure three different things: the compiled component, the complete bundle you distribute, and the installed runtime plus app. Source and assets can make the bundle much larger than its component. A shared runtime is not free disk space; its cost is paid once and must appear in first-app comparisons.</p>
<p>The <a href="/reports/">measurement reports</a> identify workloads and accounting boundaries. Do not extrapolate a notes workload into a universal speed, memory or file-size claim.</p>"""),
            ("Check whether your project fits", """<p>Krate's aim is software distribution for developers, not a fixed list of toy apps. What determines today's fit is the available API surface: UI, data, networking, media and OS integration. Check the <a href="/docs/limits.html">current limits</a> and use the <a href="/docs/porting.html">porting guide</a> to inventory dependencies before committing to a migration.</p>"""),
        ],
    },
    {
        "slug": "share-an-app-made-with-ai.html",
        "title": "How to share an AI-built desktop app | Krate",
        "description": "Create a Krate app with AI, inspect its permissions and share the .krate file. What the author needs, what the recipient installs and what to test first.",
        "h1": "Share the app, not your development setup",
        "lead": "If an AI-built application uses Krate's interfaces, you can package it as one .krate file and send it to someone on macOS, Windows or Linux. They need a compatible Krate runtime.",
        "entry_actions": [("Share a .krate file", "#send-the-file-directly"),
                          ("Make one in Studio", "/studio/")],
        "entry_note": "Already have a <code>.krate</code> file? Jump to the handoff steps. If your AI generated a website, Python script or another kind of app, start with the <a href=\"/docs/porting.html\">porting guide</a>; renaming the file does not convert it.",
        "sections": [
            ("Choose the authoring path", """<p><a href="/studio/">Krate Studio</a> provides a graphical way to make and revise apps with AI. Developers can also use the CLI or write the code themselves. AI is an authoring option, not a runtime requirement.</p>
<p>The local CLI example below requires Rust/component build tools and an installed, authenticated Claude Code CLI. Your provider's subscription or API charges are separate. Check the <a href="/docs/quickstart.html">quickstart</a> for installation and platform requirements.</p>
<pre class="answer-cmd">krate doctor
krate ai
krate create "a regex tester with a pattern box and live matches" --agent claude --output regex.krate</pre>
<p>Use <code>krate create --help</code> for your installed version's options.</p>"""),
            ("Test before you send it", """<pre class="answer-cmd">krate run regex.krate --dump-caps
krate run regex.krate --prompt</pre>
<p>The first command inspects capabilities without executing the component. The second runs it after permission review. Try valid and invalid inputs, resizing, save/reopen behavior and permission denial. Build and first-frame checks cannot establish that every feature works.</p>
<p>The normal authoring path includes editable source. Review the bundle for secrets, private sample data and third-party licensing obligations before sharing. Do not assume that generated code is correct or appropriately licensed just because it compiles.</p>"""),
            ("Send the file directly", """<p>Once you have <a href="#test-before-you-send-it">tested the app</a>, email the <code>.krate</code> file, put it in a shared folder or send it through a chat that accepts files. Include the runtime version you tested, a short description and the task the recipient can try. Optional hosted publishing is not required.</p>
<p>The recipient follows <a href="/open/">the open-a-file instructions</a>, installs the runtime for their own system, then inspects and opens the file. They do not need your AI account, source checkout or Rust toolchain. GUI file association depends on the installed runtime/opener; the CLI provides an explicit path:</p>
<pre class="answer-cmd">krate run regex.krate --dump-caps
krate run regex.krate --prompt</pre>
<p>Sending an app does not automatically send its separate saved data, copy credentials or synchronize accounts. If it needs a network service, document that dependency.</p>"""),
            ("Publishing is an optional separate step", """<p>A hub can host the bundle so you share a URL. Publishing uploads the file and may list it publicly, depending on the selected options. Read the current command's authentication, hub and listing requirements first:</p>
<pre class="answer-cmd">krate publish --help
krate publish regex.krate</pre>
<p>Do not publish private code or user data by accident. Direct file sharing remains available without a hosted publishing service.</p>"""),
            ("What the recipient can trust", """<p>A capability declaration describes requested access, not a guarantee of good behavior. Approved file or network access can still be misused within its scope. Review the source and permissions and start with apps from people you trust. See <a href="/run-ai-generated-code-safely.html">the security boundaries</a> and <a href="/portable-desktop-app-format.html">how to verify the same artifact across systems</a>.</p>"""),
        ],
    },
    {
        "slug": "run-ai-generated-code-safely.html",
        "title": "Inspect permissions before running AI-built apps | Krate",
        "description": "How Krate's capability model limits host access, how to inspect an app before running it, and what permissions cannot prove about AI-generated software.",
        "h1": "Inspect what an app built with AI can access",
        "lead": "Krate applications start without file or network access. They use Krate interfaces and receive approved capabilities. That narrows host access; it does not prove the code is correct or harmless.",
        "entry_actions": [("Inspect an app's permissions", "#read-the-permission-request-before-execution"),
                          ("Install the runtime", "/docs/quickstart.html#get-krate")],
        "sections": [
            ("Read the permission request before execution", """<p>For a local bundle from a source you trust, inspect its capability information without executing the component:</p>
<pre class="answer-cmd">krate run app.krate --dump-caps</pre>
<p>Check the requested file scope, network destinations and media/device access against the app's purpose. A document viewer asking to upload data needs an explanation. The capability list is not a transcript of everything the code might do.</p>"""),
            ("Review each grant when opening", """<pre class="answer-cmd">krate run app.krate --prompt</pre>
<p>The runtime checks host operations against session capabilities. Required capabilities that are not granted prevent the corresponding launch from proceeding; optional behavior and failure handling still need application testing. Avoid <code>--auto-grant</code> for an app you have not reviewed.</p>
<p>A grant can permit a damaging operation inside its allowed scope. Giving an app write access to a folder is not the same as proving it will preserve the contents. Use disposable copies of important data when evaluating software.</p>"""),
            ("Where enforcement happens", """<p>The guest is a WebAssembly component. The normal app profile uses Krate's WIT interfaces rather than ambient host APIs; import validation and host-side capability checks are part of the boundary. File, network and UI implementations live in the native runtime, not in a permission dialog that the guest controls.</p>
<p>Explore the <a href="https://github.com/incyashraj/krate/tree/main/wit">interface definitions</a>, <a href="https://github.com/incyashraj/krate/tree/main/crates/policy">policy implementation</a> and <a href="/docs/architecture.html">architecture guide</a>. Packaging/import checks and runtime enforcement have different jobs: passing the first is not an audit of the second.</p>"""),
            ("What is outside the guarantee", """<ul><li>Application correctness, data integrity and honest behavior inside granted access.</li>
<li>The security of your machine, coding agent, dependency installer or build toolchain.</li>
<li>Protection against every runtime, compiler, driver or host-adapter vulnerability.</li>
<li>A claim of production hardening against deliberately hostile third-party code.</li></ul>
<p>Building downloaded source is a separate trust decision from running a packaged guest: build scripts and external authoring tools are not automatically inside the app sandbox. Review dependencies and use an appropriately isolated development environment.</p>"""),
            ("A practical review checklist", """<ol><li>Obtain the bundle and source from an identifiable publisher.</li><li>Record its version and hash; a hash only helps when compared with a trusted reference.</li><li>Inspect permissions before executing it.</li><li>Try it with non-sensitive data and the narrowest useful access.</li><li>Test what happens when access is refused.</li><li>Recheck the artifact and permission request after changes.</li></ol>
<p>See the <a href="/docs/limits.html">current security limitations</a> and the project's <a href="https://github.com/incyashraj/krate/blob/main/SECURITY.md">security reporting policy</a>. Krate is not a reason to run unknown internet code casually.</p>"""),
        ],
    },
    {
        "slug": "desktop-app-distribution.html",
        "title": "Krate vs Electron vs Tauri: desktop app distribution",
        "description": "Electron and Tauri share code across platforms. Krate shares the app file. Compare desktop packaging, runtime requirements and a tested Mac, Windows and Linux example.",
        "h1": "Krate vs Electron vs Tauri: what do you ship?",
        "lead": "Electron and Tauri let you share a codebase and ship platform-specific applications. Krate lets you ship one .krate file that runs on Mac, Windows and Linux through native Krate runtimes. The difference is the file you hand to your users.",
        "entry_actions": [("Try the same-file example", "/docs/quickstart.html#try-the-chart-sample"),
                          ("Check your project's fit", "/docs/porting.html")],
        "sections": [
            ("One app, two shipping workflows", """<p>When you release an update, what needs to reach each user? In the usual packaged Electron or Tauri workflow, you distribute the build for their platform. With Krate, you distribute the same application artifact to all three.</p>
<figure class="shipping-flow">
<figcaption>Typical desktop packaging, compared with Krate's shared-runtime model. Read each row from build to delivery.</figcaption>
<div class="shipping-lane"><h3>Electron / Tauri</h3><ol>
<li><span>1. Develop</span>Shared application code</li>
<li><span>2. Package</span>Platform-specific applications</li>
<li><span>3. Deliver</span>macOS package<br>Windows package<br>Linux package</li>
</ol></div>
<div class="shipping-lane shipping-lane-krate"><h3>Krate</h3><ol>
<li><span>1. Develop</span>App using Krate's interfaces</li>
<li><span>2. Package</span>One <code>app.krate</code> file</li>
<li><span>3. Deliver</span>The same file to Mac, Windows and Linux users</li>
</ol></div>
</figure>
<p>Krate users install the native runtime for their system once, then use it to open compatible .krate apps. The runtime handles the platform-specific implementation. The application file stays the same.</p>
<p>See Electron's <a href="https://www.electronjs.org/docs/latest/tutorial/application-distribution">application packaging</a> and Tauri's <a href="https://tauri.app/distribute/">distribution formats</a>. Electron's <code>app.asar</code> can package application source, but it is delivered inside a platform-specific Electron distribution; it is not a standalone cross-OS executable.</p>"""),
            ("Compare the distribution boundary", """<div class="answer-table-wrap" role="region" aria-label="Distribution comparison" tabindex="0"><table class="answer-table">
<caption>Desktop distribution models, reviewed 20 September 2026</caption>
<thead><tr><th scope="col">Question</th><th scope="col">Electron</th><th scope="col">Tauri</th><th scope="col">Krate</th></tr></thead>
<tbody>
<tr><th scope="row">Application artifact</th><td>Platform-specific packaged application.</td><td>Platform-specific application bundle or installer.</td><td>One .krate application bundle for compatible runtimes.</td></tr>
<tr><th scope="row">UI/runtime model</th><td>Chromium and Node.js in Electron's process model.</td><td>Web frontend in an OS WebView with a Rust backend.</td><td>WebAssembly guest using Krate UI and host interfaces.</td></tr>
<tr><th scope="row">Recipient prerequisite</th><td>The packaged application and its platform requirements.</td><td>The packaged application and platform/WebView requirements.</td><td>A compatible native Krate runtime installed for that system.</td></tr>
<tr><th scope="row">Existing app migration</th><td>Fits browser/Node-based applications.</td><td>Fits web UI with Rust/native integration.</td><td>Port logic and adapt UI/host dependencies to supported Krate APIs.</td></tr>
</tbody></table></div>
<p>Electron's <a href="https://www.electronjs.org/docs/latest/tutorial/distribution-overview">distribution guide</a> covers packaging, signing, publishing and updates; its <a href="https://www.electronjs.org/docs/latest/tutorial/process-model">process model</a> explains Chromium and Node. Tauri documents <a href="https://tauri.app/distribute/">platform-specific distribution</a> and its <a href="https://tauri.app/concept/architecture/">WebView/Rust architecture</a>. Tauri does not bundle Chromium like Electron.</p>"""),
            ("Can I send exactly the same file to all three systems?", """<p>Yes, with Krate: build for Krate's interfaces and send the same .krate file to users with compatible Mac, Windows or Linux runtimes. There is no separate application build per OS. You still test the app's behavior on the systems you support.</p>
<p>Try <a href="/docs/quickstart.html#try-the-chart-sample">the downloadable Chart example</a>. Its <a href="/portable-desktop-app-format.html#try-one-file-tested-on-all-three-systems">source, full-file checksum and three-OS test evidence</a> let you check the claim yourself. No Rust toolchain or AI account is needed to run the file.</p>"""),
            ("How to decide for your project", """<p>If your application depends on a browser DOM or Node ecosystem, account for that investment before moving away from Electron. If you want a web UI plus custom Rust/native integrations, examine Tauri's APIs and deployment requirements. Neither choice means rewriting the entire application independently for every OS.</p>
<p>Evaluate Krate when distributing the same application file matters and your required features map to its interfaces. Start with <code>krate port ./my-project</code>, then validate the findings against the <a href="/docs/limits.html">current capability limits</a>. A scan is evidence for planning, not proof the port will preserve every feature.</p>
<p>For a specific app, compare a representative end-to-end task first. If an essential OS integration is missing, stay with a suitable platform or contribute that capability before migrating. See the <a href="/docs/porting.html">porting checklist</a>.</p>"""),
            ("Compare costs at the same boundary", """<p>With Krate, the runtime is installed once and shared by compatible apps. Measure the first installation as runtime plus app; measure each additional app separately. For every option, record the complete download, installed footprint and app data.</p>
<p>For performance, hold the task and inputs constant, record versions and hardware, distinguish cold/warm startup and count all relevant processes. Native dependencies, renderer work and application design can dominate. The <a href="/reports/">Krate reports</a> describe particular workloads; they are not benchmarks of all Electron or Tauri applications.</p>"""),
            ("What happens when the app changes?", """<p>Build the next .krate file and distribute it to your users. They can open that file with a compatible runtime; using the format does not require a hosted app store. Updates to the native runtime are separate from updates to your app.</p>
<p>Compatibility, user-data migration and testing remain part of releasing software. Krate changes the artifact you distribute, not those responsibilities. Start with a representative feature and real data, then use the <a href="/docs/porting.html">porting checklist</a> to work through the rest of your application.</p>"""),
        ],
    },
]


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

THREE_OS_REPLAY = f"""<p>Ten ported apps, as the same committed <code>.krate</code> files, passed their replay checks on macOS (Apple silicon), Ubuntu (x86_64) and Windows (x86_64) GitHub runners with runtime 0.5.4 on 29 September 2026. An eleventh app has no replay check defined yet and is skipped. Records: <a href="{E3}macos.tsv">macOS</a>, <a href="{E3}ubuntu.tsv">Ubuntu</a>, <a href="{E3}windows.tsv">Windows</a>. These are automated headless checks, not hands-on testing of every feature. Runtimes are also published for Intel Macs, ARM64 Windows and arm64 Linux; those are not in these checks yet.</p>"""

def comparison_table(caption, label, head, rows):
    h = "".join(f'<th scope="col">{c}</th>' for c in head)
    body = "\n".join("<tr>" + f'<th scope="row">{r[0]}</th>' + "".join(f"<td>{c}</td>" for c in r[1:]) + "</tr>" for r in rows)
    return (f'<div class="answer-table-wrap" role="region" aria-label="{label}" tabindex="0"><table class="answer-table">\n'
            f"<caption>{caption}</caption>\n<thead><tr>{h}</tr></thead>\n<tbody>\n{body}\n</tbody></table></div>")

# Shipping-led pages (K-SEO, 6 October 2026). Every page answers in its lead,
# names the other tools fairly and links official sources for their facts.
PAGES += [
    {
        "slug": "how-to-distribute-a-desktop-app.html",
        "title": "How to distribute a desktop app to Windows, macOS and Linux | Krate",
        "description": "Usually: a package per OS, code signing, notarization and builds per CPU. With Krate: one .krate file that opens on all three through a runtime installed once.",
        "h1": "How to distribute a desktop app to Windows, macOS and Linux",
        "lead": "Most teams build a separate package for each operating system, often one per processor too, then sign, notarize and host each download. Krate changes that step: you ship one <code>.krate</code> file, your users install the Krate runtime once, and the same file opens on macOS, Windows and Linux in a sandbox, with only the access they grant.",
        "reviewed": "2026-10-06",
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
            ("The Krate route", """<ol>
<li><strong>Make the app.</strong> Describe it to an AI coding agent in <a href="/studio/">Krate Studio</a>, or write it in Rust against the Krate SDK. Studio can also port a project you already have: the AI rewrites it against Krate's interfaces, so read the plan it shows before you start.</li>
<li><strong>Pack one file.</strong> The result is one <code>.krate</code>: a manifest that names the access the app wants, a WebAssembly component that holds the app, and optionally its assets and source. It contains no Intel or ARM machine code, so there is nothing to build per processor.</li>
<li><strong>Send it like a document.</strong> Email it, drop it in a shared folder or a chat, or publish it and send a link. Every recipient gets the same bytes.</li>
<li><strong>Your users install Krate once.</strong> Krate for Mac is signed and notarized by Apple. The Windows runtime is not code-signed yet, so SmartScreen asks once when it is installed. After that, each new app is only its own file.</li>
<li><strong>They decide what it may touch.</strong> An app starts without file or network access and asks for what it needs. The runtime checks protected calls against what was allowed; the known gaps are on the <a href="/docs/limits.html">limits page</a>. See <a href="/run-ai-generated-code-safely.html">how to inspect an app before running it</a>.</li>
</ol>"""),
            ("What you stop doing, and what you still do", """<p><strong>You stop</strong> producing a separate app package for each operating system and processor, signing and notarizing each app for each platform, and asking people to run an installer that gets their full permissions.</p>
<p><strong>You still</strong> test the app on the systems you support, write it against Krate's interfaces (Rust today), check the <a href="/docs/limits.html">current limits</a> and tell recipients where to get the runtime. Runtime updates are separate from your app's updates. Krate does not turn an existing <code>.exe</code> or <code>.app</code> into a <code>.krate</code>.</p>"""),
            ("Check it yourself", f"""<p>Download <a href="{CHART}">chart.krate</a>, one file of 11,455 bytes with SHA-256 <code>3d290c48f74936d5cdb45ceb0ee945bd0f7b5b0b21f87f79917bced3b4ca73c3</code>. Follow <a href="/docs/quickstart.html#try-the-chart-sample">the quickstart</a> to see what it asks for and run it on any of the three systems.</p>
{THREE_OS_REPLAY}"""),
            ("When another route fits better", """<ul>
<li>Your app also needs phones or the web: Tauri, Flutter or a web app cover more platforms today.</li>
<li>Your app depends on the browser DOM or Node.js packages: Electron runs that code as it is.</li>
<li>You need native libraries, subprocesses or a demanding 3D renderer; see the <a href="/docs/limits.html">limits page</a>.</li>
</ul>
<p>Krate fits desktop tools, internal utilities, dashboards, editors, small games and apps made with AI, where getting one file to people safely matters more than reaching every platform.</p>"""),
        ],
        "faq": [
            ("Can one file run on Windows, macOS and Linux?", "<p>A normal native executable can't, because each system uses its own executable format. A <code>.krate</code> can, because it is not a native executable: it holds a WebAssembly component that the installed Krate runtime runs on each system.</p>"),
            ("Do the people I send it to need to install anything?", '<p>Yes, once: the Krate runtime for their system, from the <a href="/download/">download page</a>. After that, every Krate app is a single file they open.</p>'),
            ("Do I need a code signing certificate to ship a .krate?", '<p>No. You don\'t sign and notarize a package per operating system, because the file isn\'t one. You can still sign a <code>.krate</code> with your own key so people can check it came from you. See <a href="/desktop-app-code-signing.html">code signing and Krate</a>.</p>'),
            ("Can I ship an app I made with AI this way?", '<p>Yes, if it is a Krate app. Make it in Krate Studio with the AI agent you already use, or open an existing project in Studio and port it. See <a href="/share-an-app-made-with-ai.html">how to share an AI-built desktop app</a>.</p>'),
        ],
    },
    {
        "slug": "krate-vs-electron.html",
        "title": "Krate vs Electron: an Electron alternative that ships one file",
        "description": "Electron packages Chromium and Node.js into each app, per OS. Krate ships one .krate file that a shared runtime opens on Mac, Windows and Linux. Where each fits.",
        "h1": "Krate vs Electron: one app file instead of a browser in every app",
        "lead": "Krate is an Electron alternative for desktop apps you want to ship as one file. Electron packages Chromium and Node.js into a separate app for each operating system. Krate ships one <code>.krate</code> file that the Krate runtime, installed once, opens on macOS, Windows and Linux. Electron is far more mature and runs your existing web code; Krate apps are written against Krate's own interfaces, in Rust today.",
        "reviewed": "2026-10-06",
        "entry_actions": [("See the comparison", "#side-by-side"),
                          ("Check your project's fit", "/docs/porting.html")],
        "sections": [
            ("Side by side", comparison_table("Electron and Krate, reviewed 6 October 2026 against Electron's official docs", "Electron and Krate compared",
                ["Question", "Electron", "Krate"], [
                ["What you ship", "A packaged app for each operating system, each carrying its own Chromium and Node.js.", "One <code>.krate</code> file for all three, opened by a runtime the user installs once and every Krate app shares."],
                ["App code", "HTML, CSS and JavaScript in Chromium, with Node.js in the main process.", "A WebAssembly component that calls Krate's interfaces. Rust today."],
                ["Builds per OS and CPU", "One package per target platform.", "One file. The runtime is the only per-platform piece, and Krate builds it."],
                ["Signing", "Sign for Windows and macOS, and notarize for macOS, for each release.", "No per-OS signing of the app file. Optional Krate signature with your own key."],
                ["Default access", "Renderers are sandboxed by default since Electron 20. The main process is not sandboxed and has Node.js.", "The app starts without file or network access. The person running it grants or refuses each request."],
                ["Platforms", "Windows, macOS and Linux.", "macOS, Windows and Linux. iOS and Android exist only as reference ports."],
                ["Maturity", "Established, with a large ecosystem and many well-known apps.", "Young (0.5.x), with a small ecosystem."],
            ]) + f"""
<p>Electron facts: <a href="{EL_PROC}">process model</a>, <a href="{EL_SANDBOX}">process sandboxing</a>, <a href="{EL_SIGN}">code signing</a> and <a href="{EL_DIST}">distribution overview</a>.</p>"""),
            ("What one file changes for shipping", """<p>With Electron, each release is a set of platform builds that you sign, notarize and host. With Krate, each release is one new file. People who already have Krate open it straight away; everyone else installs the runtime once first. Because Krate apps share the runtime, a second or third app does not bring another copy of a browser.</p>
<p>Count the same things when you compare size: for a first-time recipient, the runtime plus the app; for everyone after that, the app file alone. The <a href="/reports/">measurements page</a> shows one scoped comparison with MarkText, an Electron editor, and says what it does and doesn't cover.</p>"""),
            ("Who decides what the app can touch", """<p>In Electron, the developer decides what the main process exposes to the sandboxed renderers, and the main process itself runs with the user's permissions. That is a sound design for code you wrote, but the person running the app has no say in it.</p>
<p>In Krate, the app lists what it wants in its manifest and the person running it approves or refuses. The runtime checks protected calls against what was allowed. Krate is young: the known gaps are on the <a href="/docs/limits.html">limits page</a>, and it does not yet claim hardening against deliberately hostile code.</p>"""),
            ("When to stay with Electron", """<ul>
<li>You have a web or Node.js codebase and want to keep running it as it is.</li>
<li>You depend on the DOM, npm packages or Chromium's consistent rendering on every system.</li>
<li>You need mature auto-update tooling or a large plugin ecosystem.</li>
</ul>"""),
            ("Moving an Electron app to Krate", """<p>Start with a scan. It reads your source without building or running it and lists what maps to Krate and what doesn't:</p>
<pre class="answer-cmd">krate port ./my-electron-app</pre>
<p>A port is a rewrite in Rust against the Krate SDK, not a repackage. DOM and Node.js APIs do not carry over. Krate Studio can do the rewrite with AI and shows its plan first. Begin with one representative feature and read the <a href="/docs/porting.html">porting guide</a>.</p>"""),
        ],
        "faq": [
            ("Is Krate a drop-in replacement for Electron?", "<p>No. Electron runs your HTML, CSS and JavaScript as they are. A Krate app is written against Krate's interfaces, in Rust today, so moving an Electron app is a port, not a repackage.</p>"),
            ("Do Krate users need Chromium?", "<p>No. Krate apps don't run in a browser engine. Users install the Krate runtime once, and every Krate app shares it.</p>"),
            ("Can I keep writing JavaScript?", "<p>Not for a shipped Krate app today: Rust is the only finished SDK. Krate Studio can write the Rust for you from a description, or port a JavaScript project by rewriting it.</p>"),
        ],
    },
    {
        "slug": "krate-vs-tauri.html",
        "title": "Krate vs Tauri: one .krate file or a bundle for each OS",
        "description": "Tauri builds a WebView app and an installer for each OS. Krate ships one .krate file that opens on Mac, Windows and Linux through a runtime installed once.",
        "h1": "Krate vs Tauri: what you ship and who holds the permissions",
        "lead": "Tauri and Krate are both built in Rust and both take permissions seriously, but they ship differently. Tauri builds a platform-specific installer or bundle for each operating system, with your web frontend running in the system WebView. Krate ships one <code>.krate</code> file that the installed Krate runtime opens on macOS, Windows and Linux. Tauri is more mature and also targets Android and iOS; Krate is desktop only today.",
        "reviewed": "2026-10-06",
        "entry_actions": [("See the comparison", "#side-by-side"),
                          ("Try a .krate file", "/docs/quickstart.html#try-the-chart-sample")],
        "sections": [
            ("Side by side", comparison_table("Tauri and Krate, reviewed 6 October 2026 against Tauri's official docs", "Tauri and Krate compared",
                ["Question", "Tauri", "Krate"], [
                ["What you ship", "An installer or bundle per platform: DMG or app bundle, a Windows installer, AppImage, deb, RPM, Flatpak or Snap.", "One <code>.krate</code> file for macOS, Windows and Linux."],
                ["Interface", "Your web frontend in the system WebView: WebView2, WKWebView or webkit2gtk.", "Krate's own UI interfaces. No WebView."],
                ["App logic", "Rust commands, with plugins in Rust, Swift or Kotlin.", "A WebAssembly component. Rust today."],
                ["Permissions", "Capability files the developer writes grant or deny commands per window.", "The app requests access in its manifest; the person running it grants or refuses."],
                ["Signing", "Required on most platforms. Direct macOS downloads also need notarization.", "No per-OS signing of the app file. Optional Krate signature with your own key."],
                ["Platforms", "Windows, macOS, Linux, Android and iOS.", "macOS, Windows and Linux. iOS and Android exist only as reference ports."],
                ["Maturity", "Version 2, stable since 2024.", "Young (0.5.x)."],
            ]) + f"""
<p>Tauri facts: <a href="{TA_DIST}">distribution</a>, <a href="{TA_CAPS}">capabilities</a> and <a href="{TA_WEBVIEW}">WebView versions</a>.</p>"""),
            ("Who holds the permissions", f"""<p>Tauri's capabilities limit what the frontend in the WebView may call. They are chosen by the developer, and <a href="{TA_CAPS}">Tauri's own docs</a> say they do not protect against malicious or insecure Rust code in the app itself. They protect users from a compromised frontend.</p>
<p>In Krate, the whole app is the guest. It cannot call the operating system directly; it calls Krate interfaces, and the runtime checks protected calls against what the person running it allowed. The model is young and its gaps are on the <a href="/docs/limits.html">limits page</a>.</p>"""),
            ("Shipping a release", """<p>With Tauri, a release means building on each platform, usually in a CI job per system, signing for each platform, and hosting and updating each package. With Krate, a release is one new <code>.krate</code> file that you send or publish. Recipients need a compatible runtime, and runtime updates are separate from yours.</p>"""),
            ("When Tauri is the better choice", """<ul>
<li>You want a web frontend, or already have one.</li>
<li>You need Android or iOS from the same project.</li>
<li>You call native libraries from Rust. A <code>.krate</code> cannot link a native library.</li>
<li>You need its plugin ecosystem.</li>
</ul>"""),
            ("Moving a Tauri app to Krate", """<p>Inventory your Rust commands, plugins and WebView interactions, then run the scan, which reads source without building it:</p>
<pre class="answer-cmd">krate port ./my-tauri-app</pre>
<p>Pure Rust logic may carry over. The WebView interface and native plugins need adapting to Krate's widgets, canvas and host interfaces. See the <a href="/docs/porting.html">porting guide</a>.</p>"""),
        ],
        "faq": [
            ("Is Krate built on Tauri?", "<p>No. Krate is its own runtime, built on Wasmtime. It does not use a WebView.</p>"),
            ("Can a Tauri app run in Krate unchanged?", "<p>No. A Tauri app is a native program with a web frontend. A Krate app is a WebAssembly component written against Krate's interfaces, so moving one is a port.</p>"),
            ("Which one makes smaller downloads?", "<p>It depends on what you count. A Tauri app relies on the system WebView, so its package can be small. A Krate app file is small too, but a first-time recipient also downloads the Krate runtime once. Compare runtime plus app for the first app, and the app alone after that.</p>"),
        ],
    },
    {
        "slug": "desktop-app-or-web-app.html",
        "title": "Desktop app or hosted web app? Ship without a server | Krate",
        "description": "Hosting a web app avoids installers but needs a server and a browser. A .krate is a desktop app in one file that works offline and asks before it uses your files.",
        "h1": "Ship a desktop app or host a web app?",
        "lead": "Host a web app when people need it on phones or without installing anything. Ship a desktop app when it should open like a program, work offline and handle local files. A <code>.krate</code> gives you the desktop option without a package per OS: one file that opens on macOS, Windows and Linux, at the cost of a one-time Krate runtime install for each person.",
        "reviewed": "2026-10-06",
        "entry_actions": [("See the comparison", "#side-by-side"),
                          ("How to distribute a desktop app", "/how-to-distribute-a-desktop-app.html")],
        "sections": [
            ("Side by side", comparison_table("A hosted web app and a .krate desktop app, reviewed 6 October 2026", "Web app and Krate app compared",
                ["Question", "Hosted web app", "Krate app"], [
                ["What people get", "A page in a browser tab, or an installed web app where the browser supports it.", "A desktop app in its own window."],
                ["What you run", "A server or hosting account, for as long as people use the app.", "Nothing. The file runs on their computer. Publishing to a hub is optional."],
                ["Offline", "Only with extra work, such as a service worker.", "Works without a connection unless the app itself needs the network."],
                ["Local files", f'Depends on the browser. The File System Access picker is <a href="{MDN_FSA}">not available in all major browsers</a>.', "Through a file dialog or a folder the person grants."],
                ["Updates", "Every visitor gets the new version on reload.", "You send or publish a new file."],
                ["Phones", "Yes.", "Not yet. Desktop only today."],
                ["What people install", "Nothing.", "The Krate runtime, once."],
            ])),
            ("Hosting has running costs too", """<p>A hosted app needs a domain, a hosting bill, uptime, sign-in and storage for user data, and it keeps people's data on your server, which brings privacy duties. A Krate app keeps its saved data on the person's computer unless it is granted network access and sends it somewhere. If it needs a service, you still run that service, but the app itself does not need hosting.</p>"""),
            ("Apps made with Lovable, Bolt, v0 and similar tools", """<p>These tools mostly produce web apps. You can host the published URL, or wrap it with Electron, Tauri or a wrapper service, which gives you a package per OS to build, sign and host. Krate does not wrap websites. To ship such an app as a <code>.krate</code>, open the project in Krate Studio and port it: the AI rewrites it against Krate's interfaces and shows its plan first. The DOM does not carry over. See <a href="/share-an-app-made-with-ai.html">how to share an AI-built desktop app</a>.</p>"""),
            ("When the web is the right answer", """<ul>
<li>People need it on phones, or cannot install anything.</li>
<li>Many people work on the same live data.</li>
<li>Search engines need to index the content.</li>
<li>Your team and your code are already on the web.</li>
</ul>"""),
        ],
        "faq": [
            ("Can I turn my website into a desktop app?", "<p>You can wrap it with Electron or Tauri, which gives you a package per OS to build and sign. Krate doesn't wrap websites: a Krate app is written against Krate's interfaces, and Krate Studio can port a web project by rewriting it.</p>"),
            ("Do Krate apps need an internet connection?", "<p>No, unless the app itself uses the network, and then only to the hosts the person allows.</p>"),
            ("Can people open a .krate on their phone?", "<p>Not yet. iOS and Android exist in Krate's source as reference ports and are not shipping.</p>"),
        ],
    },
    {
        "slug": "desktop-app-code-signing.html",
        "title": "Do you need code signing to ship a desktop app? | Krate",
        "description": "Native installers need a Developer ID and notarization on macOS and a trusted signature on Windows. A .krate is not a native executable. What that changes.",
        "h1": "Do you need code signing to ship a desktop app?",
        "lead": "For a native installer, in practice yes: macOS expects a Developer ID signature and Apple notarization for apps downloaded outside the App Store, and Windows warns about unsigned downloads, and about new signed ones until they build reputation. A <code>.krate</code> is not a native executable, so you don't sign and notarize a package for each system. Your users install the Krate runtime once, and it opens your file and shows what the app asks for.",
        "reviewed": "2026-10-06",
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
            ("How it works with a .krate", """<p>The operating system checks the Krate runtime once, when it is installed. Krate for Mac is signed and notarized by Apple. The Windows runtime is not code-signed yet, so SmartScreen asks once at install, and Windows 11 machines with Smart App Control may block it.</p>
<p>Your app is a file the runtime opens, not a program the operating system launches, so there is no per-OS package of yours to sign or notarize. When someone opens it, Krate shows what it asks for, and the app gets only what they allow.</p>"""),
            ("Signing a .krate with your own key", """<p>You can sign a bundle so recipients can check it came from you and has not changed. The key is an Ed25519 key you create; it stays on your machine, and signing and checking work offline with no account:</p>
<pre class="answer-cmd">krate sign app.krate --key publisher.key --generate-key --namespace com.example.app</pre>
<p><code>--generate-key</code> writes a new key the first time and refuses to overwrite one. Run <code>krate sign --help</code> for your version's options. This signature ties releases to your key; it is not a certificate authority's check of your legal identity.</p>"""),
            ("What you still own", """<ul>
<li>Telling people where to get Krate, and that Windows asks once at install today.</li>
<li>Publishing your app's hash or signing key somewhere they trust, so they can check what they received.</li>
<li>Testing on the systems you support, within the <a href="/docs/limits.html">current limits</a>.</li>
</ul>"""),
        ],
        "faq": [
            ("Can I distribute a Windows app without a code signing certificate?", "<p>Yes, but users see a SmartScreen warning and must choose to run it anyway, and some managed or Smart App Control machines block it. With Krate you ship a <code>.krate</code> instead of an <code>.exe</code>; Windows checks the Krate runtime when it is installed, and that runtime is unsigned today.</p>"),
            ("Do I need an Apple Developer account to share a Mac app?", "<p>For a native Mac app downloaded from the web, in practice yes: notarization needs a Developer ID from the paid Apple Developer Program. A <code>.krate</code> is opened by Krate for Mac, which is signed and notarized, so you don't need your own Apple account to share one.</p>"),
            ("Is a signed app safe to run?", "<p>A signature tells you who made it and that it was not changed. It does not limit what the app can do once it runs. Krate adds a second check: the app asks before it gets your files or the network.</p>"),
        ],
    },
    {
        "slug": "desktop-app-shipping-faq.html",
        "title": "Shipping a desktop app: straight answers to common questions | Krate",
        "description": "How to ship one app to Windows, macOS and Linux, share an app built with AI, skip per-OS builds, run untrusted apps more safely and pick an Electron or Tauri alternative.",
        "h1": "Shipping a desktop app: questions and straight answers",
        "lead": "Short answers to the questions people ask once they have built an app and need to get it onto other people's computers. Each answer starts with the answer, then links to the longer page.",
        "reviewed": "2026-10-06",
        "entry_actions": [("How to distribute a desktop app", "/how-to-distribute-a-desktop-app.html"),
                          ("Install Krate", "/download/")],
        "entry_note": 'These answers describe Krate 0.5.4. Krate is young, and the <a href="/docs/limits.html">limits page</a> lists what it cannot do yet. Where another tool is the better fit, the answer says so.',
        "sections": [],
        "faq": [
            ("How do I distribute a desktop app to Windows, macOS and Linux?", '<p>Either build, sign and host a package for each system, or ship one <code>.krate</code> file that opens on all three through the Krate runtime. <a href="/how-to-distribute-a-desktop-app.html">Both routes, step by step</a>.</p>'),
            ("Can I ship one app file to Windows, Mac and Linux?", '<p>Yes, as a <code>.krate</code>: the same bytes open on all three once the person has installed the Krate runtime. A normal native executable cannot do this. <a href="/portable-desktop-app-format.html">How the format works</a>.</p>'),
            ("What is a good Electron alternative?", '<p>It depends on what you need. Tauri, Wails and Neutralino keep a web frontend in the system WebView; Flutter and Qt draw their own interface; Krate ships one sandboxed <code>.krate</code> file for all three desktop systems. <a href="/krate-vs-electron.html">Krate vs Electron</a>.</p>'),
            ("Is there a Tauri alternative that doesn't need a build per OS?", '<p>Krate. Tauri builds an installer or bundle for each operating system; Krate ships one file and keeps the per-OS work in a runtime that Krate maintains. <a href="/krate-vs-tauri.html">Krate vs Tauri</a>.</p>'),
            ("How do I share an app I built with AI as a desktop app?", '<p>If it is a Krate app, send the <code>.krate</code> file; the person installs Krate once and opens it. If your AI tool built a website, host it, wrap it per OS with Electron or Tauri, or port it to Krate in Krate Studio. <a href="/share-an-app-made-with-ai.html">Sharing an AI-built app</a>.</p>'),
            ("Can I ship a cross-platform app without code signing?", '<p>With a <code>.krate</code>, you don\'t sign a package per OS, because the file is opened by the Krate runtime. Native installers effectively need signing on macOS and Windows. <a href="/desktop-app-code-signing.html">Code signing and Krate</a>.</p>'),
            ("How can I run an untrusted desktop app more safely?", '<p>For an unknown native program, use the strongest isolation you have, such as a virtual machine or Windows Sandbox. For apps shipped as <code>.krate</code> files, the app starts without file or network access and asks; inspect it first with <code>krate run app.krate --dump-caps</code>. Krate is not yet hardened against deliberately hostile code. <a href="/run-ai-generated-code-safely.html">Inspecting an app before you run it</a>.</p>'),
            ("Is there a portable app format that runs on every OS?", '<p>No format runs on every system. A <code>.krate</code> runs on macOS, Windows and Linux desktops through the Krate runtime. AppImage is Linux only, Cosmopolitan\'s Actually Portable Executable runs one executable on several systems, mainly for command-line programs, and Java apps need a Java runtime. <a href="/portable-desktop-app-format.html">The .krate format</a>.</p>'),
            ("Is there a WebAssembly runtime for desktop GUI apps?", '<p>Yes. Krate runs desktop apps compiled to WebAssembly components, with windows, widgets, a 2D canvas, files, storage and networking behind permissions. It embeds Wasmtime. General runtimes such as Wasmtime, Wasmer and WasmEdge focus on servers and command-line programs. <a href="/docs/architecture.html">Krate\'s architecture</a>.</p>'),
            ("Do people need to install anything to open a .krate?", '<p>Yes, the Krate runtime, once. <a href="/download/">Download Krate</a>, or read <a href="/open/">what to do when someone sends you a .krate</a>.</p>'),
            ("Is Krate free and open source?", '<p>The runtime, CLI and SDK are free and open source under MIT OR Apache-2.0. Krate Studio is source-available under the Business Source License 1.1 and free on your own machine with your own AI. <a href="https://github.com/incyashraj/krate#license">Licence details</a>.</p>'),
            ("Does Krate run existing .exe or .app files?", '<p>No. An app has to be built or ported for Krate\'s interfaces. Krate Studio can port a project folder with AI; the <a href="/docs/porting.html">porting guide</a> explains what carries over.</p>'),
            ("What can't Krate do yet?", '<p>Phones, shipping apps in languages other than Rust, native libraries and subprocesses, and demanding 3D games. <a href="/docs/limits.html">The full list</a>.</p>'),
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
        self.assertLess(result.index("Already have a"), result.index("Choose the authoring path"))
        self.assertIn('href="#send-the-file-directly"', result)
        self.assertIn('href="#test-before-you-send-it"', result)
        self.assertIn('href="/open/"', result)
        self.assertIn("renaming the file does not convert it", result)
        self.assertIn("compatible Krate runtime", result)

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
        for scope in ("headless replay", "source-built runtimes", "not an interactive test",
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
        self.assertIn("runtime is installed once", result)
        self.assertIn("runtime plus app", result)
        self.assertIn("You still test the app", result)

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
            related = result[result.index('<section id="more-answers">'):]
            for other in PAGES:
                if other is not page:
                    self.assertIn(f'href="/{other["slug"]}"', related, page["slug"])
            self.assertNotIn(f'href="/{page["slug"]}"', related)

    def test_internal_answer_links_resolve(self):
        slugs = {p["slug"] for p in PAGES}
        for page in PAGES:
            for target in re.findall(r'href="/([a-z0-9-]+\.html)', render(page)):
                self.assertIn(target, slugs, f"{page['slug']} links /{target}")

    def test_shipping_comparisons_cite_official_sources_and_say_when_to_stay(self):
        expected = {
            "krate-vs-electron.html": ("https://www.electronjs.org/docs/latest/tutorial/code-signing",
                                       "https://www.electronjs.org/docs/latest/tutorial/sandbox", "When to stay with Electron"),
            "krate-vs-tauri.html": ("https://tauri.app/distribute/", "https://tauri.app/security/capabilities/",
                                    "When Tauri is the better choice"),
            "desktop-app-code-signing.html": ("https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/smartscreen-reputation",
                                              "https://www.electronjs.org/docs/latest/tutorial/code-signing", "What you still own"),
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
        self.assertIn("not hands-on testing of every feature", result)

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

    def test_metadata_is_escaped(self):
        page = dict(PAGES[0], title='A "quoted" <title> & more', description='Keep </script> as text')
        result = page_head(page)
        self.assertIn("&quot;quoted&quot; &lt;title&gt; &amp; more", result)
        schemas = re.findall(r'<script type="application/ld\+json">(.*?)</script>', result, re.S)
        self.assertEqual(json.loads(schemas[0])["description"], page["description"])


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
    return 0


if __name__ == "__main__":
    if "--self-test" in sys.argv:
        unittest.main(argv=[sys.argv[0]])
    else:
        raise SystemExit(main())
