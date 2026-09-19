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
import unittest
from html.parser import HTMLParser
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LANDING = ROOT / "docs" / "landing" / "index.html"
OUT = ROOT / "docs" / "answers"


def chrome():
    """Head from the landing page; nav and footer as the subpage shell.

    The head is lifted from the real landing page so meta and stylesheet
    choices cannot drift. The nav and footer are NOT scraped: the landing
    page's header is the mega-panel one, and scraping it broke silently when
    the landing was rebuilt -- find() missed, and every answer page shipped
    with no navigation at all. The subpage shell (the same subnav/subfoot as
    /faq.html) is what these pages actually want, so it is written out here.
    """
    s = LANDING.read_text()
    end = re.search(r"</head\s*>", s, re.I)
    if end is None:
        raise ValueError("Landing page has no closing head tag")
    head = s[:end.end()]
    # These live at the site root; the landing's relative links do not.
    head = head.replace('href="./', 'href="/').replace('src="./', 'src="/')

    nav = """<header class="subnav">
  <div class="wrap subnav-inner">
    <a class="brand" href="/"><img src="/krate-logo.png" alt="" width="22" height="22" /> KRATE</a>
    <nav aria-label="Primary">
      <a href="/docs/quickstart.html">Start</a>
      <a href="/docs/">Docs</a>
      <a href="/cloud/">Apps</a>
      <a href="https://github.com/incyashraj/krate">GitHub</a>
    </nav>
    <a class="pill pill-primary" href="/docs/quickstart.html#get-krate">Install</a>
  </div>
</header>"""

    foot = """<footer class="subfoot">
  <div class="wrap subfoot-inner">
    <span>© 2026 Krate</span>
    <span>
      <a href="/docs/">Docs</a>
      <a href="/reports/">Reports</a>
      <a href="/progress/">Progress</a>
      <a href="https://github.com/incyashraj/krate">GitHub</a>
      <a href="/contact/">Contact</a>
    </span>
  </div>
</footer>"""
    return head, nav, foot


# These pages inline the LANDING page's head, which does not carry the
# `.answer-cmd` rule -- that one lives in docs/landing/krate.css, a
# stylesheet these pages never load. So the command blocks shipped as bare
# <pre>: `white-space: pre`, no overflow rule, no max width. Measured at
# 390px, one `krate run ... --grant` line made the whole document 655px
# wide and the PAGE scrolled sideways, on every answer page.
#
# The commands must stay unwrapped -- a shell line broken across lines is a
# line somebody pastes wrong -- so the block scrolls inside its own box
# instead, and `max-width: 100%` stops it widening its parents.
ANSWER_CSS = """  <style>
    /* The article shell owns its layout. The homepage supplies fonts and
       colors, not styles for these differently named elements. */
    .subnav {
      position: sticky; top: 0; z-index: 40;
      background: rgba(0,0,0,.94); border-bottom: 1px solid #27272a;
      -webkit-backdrop-filter: blur(12px); backdrop-filter: blur(12px);
    }
    .subnav-inner, .subfoot-inner {
      width: min(1120px, calc(100% - 48px)); margin: 0 auto;
      display: flex; align-items: center; flex-wrap: wrap; gap: 12px 24px;
    }
    .subnav-inner { padding: 12px 0; }
    .subnav .brand {
      display: inline-flex; align-items: center; gap: 8px;
      min-height: 44px; font-size: 16px; font-weight: 500; letter-spacing: .02em;
    }
    .subnav .brand img { flex: none; }
    .subnav nav { display: flex; flex-wrap: wrap; gap: 4px; margin-left: auto; }
    .subnav nav a, .subfoot a {
      display: inline-flex; align-items: center; justify-content: center;
      min-height: 44px; padding: 10px 12px; color: #a1a1aa;
      font-size: 14px; line-height: 1.5; border-radius: 8px;
    }
    .subnav nav a:hover, .subfoot a:hover { color: #fff; background: #18181b; }
    .page-wrap {
      width: min(760px, calc(100% - 48px)); margin: 0 auto;
      padding: 64px 0 88px; color: #d4d4d8; line-height: 1.75;
    }
    .page-wrap h1, .page-wrap h2, .page-wrap h3 {
      font-family: var(--disp); color: #fafafa; font-weight: 700;
      letter-spacing: -.025em;
    }
    .page-wrap h1 { font-size: clamp(30px, 4.5vw, 46px); line-height: 1.14; margin-bottom: 20px; }
    .page-wrap h2 { font-size: clamp(22px, 3vw, 28px); line-height: 1.25; margin-bottom: 18px; }
    .page-wrap h3 { font-size: 20px; line-height: 1.35; margin: 24px 0 12px; }
    .page-wrap p { margin: 0 0 18px; }
    .page-wrap .page-lede { font-size: 18px; line-height: 1.7; color: #a1a1aa; margin-bottom: 24px; }
    .page-wrap > section { margin-top: 40px; padding-top: 32px; border-top: 1px solid #27272a; }
    .page-wrap ul, .page-wrap ol { padding-left: 24px; margin: 0 0 20px; }
    .page-wrap li { padding-left: 4px; margin-bottom: 10px; }
    .page-wrap strong { color: #fafafa; font-weight: 500; }
    .page-wrap :is(p, li) a {
      color: #c4b5fd; text-decoration: underline; text-decoration-thickness: 1px;
      text-underline-offset: 3px;
    }
    .page-wrap :is(p, li) a:hover { color: #ede9fe; }
    .page-wrap :is(p, li) code { color: #e4e4e7; font-size: .9em; }
    :is(.subnav, .page-wrap) .pill {
      display: inline-flex; align-items: center; justify-content: center;
      min-height: 44px; padding: 10px 18px; border-radius: 999px;
      border: 1px solid #3f3f46; background: #09090b; color: #e4e4e7;
      font-size: 14px; font-weight: 500; line-height: 1.4; text-align: center;
    }
    :is(.subnav, .page-wrap) .pill:hover { background: #18181b; border-color: #71717a; }
    :is(.subnav, .page-wrap) .pill-primary { background: #fafafa; border-color: #fafafa; color: #09090b; }
    :is(.subnav, .page-wrap) .pill-primary:hover { background: #d4d4d8; border-color: #d4d4d8; }
    :is(.subnav, .page-wrap, .subfoot) :is(a, pre, [tabindex]):focus-visible {
      outline: 2px solid #c4b5fd; outline-offset: 4px;
    }
    .subfoot { border-top: 1px solid #27272a; color: #a1a1aa; }
    .subfoot-inner { padding: 24px 0; justify-content: space-between; font-size: 13px; }
    .subfoot-inner > span:last-child { display: flex; flex-wrap: wrap; gap: 4px; }
    /* Command blocks: scroll inside the box, never widen the page. */
    .answer-cmd {
      margin: 0 0 18px;
      padding: 14px 16px;
      max-width: 100%;
      overflow-x: auto;
      -webkit-overflow-scrolling: touch;
      overscroll-behavior-x: contain;
      color: #7fb2ff;
      background: rgba(255, 255, 255, 0.03);
      border: 1px solid rgba(255, 255, 255, 0.1);
      border-radius: 8px;
      font-size: 13px;
      line-height: 1.65;
      white-space: pre;
    }
    /* Nothing inside the page shell may widen it either: a long URL or an
       inline <code> token is the other way a page starts scrolling. */
    .page-wrap { min-width: 0; }
    .page-wrap :is(p, li, h1, h2, h3, td) { overflow-wrap: anywhere; }
    .page-wrap section[id] { scroll-margin-top: 88px; }
    .answer-entry { margin-bottom: 28px; }
    .answer-entry > p { margin-top: 18px; font-size: 14px; color: #a1a1aa; }
    /* The two pills at the foot of the page. They wrap rather than squeeze,
       and they are a real tap target rather than a line of text: measured
       at 26px before this, against the 44px the same pill gets in the nav. */
    .answer-actions {
      display: flex;
      flex-wrap: wrap;
      gap: 12px;
      margin-top: 18px;
    }
    .answer-actions .pill {
      display: inline-flex;
      align-items: center;
      justify-content: center;
      min-height: 44px;
      padding: 10px 20px;
    }
    .answer-table-wrap { max-width: 100%; overflow-x: auto; margin: 20px 0; }
    .answer-table { width: 100%; min-width: 640px; border-collapse: collapse; font-size: 14px; }
    .answer-table caption { text-align: left; color: #a1a1aa; font-size: 13px; margin-bottom: 12px; }
    .answer-table thead th { color: #fafafa; background: #18181b; }
    .answer-table th, .answer-table td {
      padding: 12px; text-align: left; vertical-align: top;
      border-bottom: 1px solid rgba(255,255,255,.14);
    }
    @media (max-width: 760px) {
      .answer-cmd { font-size: 12.5px; padding: 12px 14px; }
      /* One per line on a phone, each full width: two pills side by side
         at this width leaves each too narrow to read comfortably. */
      .answer-actions { flex-direction: column; align-items: stretch; }
      .answer-actions .pill { width: 100%; }
    }
    @media (max-width: 640px) {
      .subnav-inner, .subfoot-inner { width: calc(100% - 32px); gap: 6px 12px; }
      .subnav nav { order: 2; flex-basis: 100%; margin: 0; justify-content: space-between; }
      .subnav-inner > .pill { margin-left: auto; }
      .page-wrap { width: calc(100% - 40px); padding: 40px 0 56px; }
      .page-wrap h1 { font-size: clamp(28px, 7.5vw, 36px); }
      .page-wrap .page-lede { font-size: 16px; }
      .page-wrap > section { margin-top: 32px; padding-top: 28px; }
      .page-wrap section[id] { scroll-margin-top: 144px; }
      .subfoot-inner { align-items: flex-start; gap: 12px; }
      .subfoot-inner > span:last-child { flex-basis: 100%; }
    }
  </style>
"""


class PageHead(HTMLParser):
    """Keep shared styles/assets, never inherit the homepage's identity.

    Attribute order, quote style and optional HTML self-closing slashes must
    not determine whether the canonical and social metadata are replaced.
    """

    def __init__(self):
        super().__init__(convert_charrefs=False)
        self.parts = []
        self.skip = None

    def handle_starttag(self, tag, attrs):
        values = {key.lower(): (value or "").lower() for key, value in attrs}
        name = values.get("name", "")
        prop = values.get("property", "")
        if tag == "title" or (tag == "script" and values.get("type") == "application/ld+json"):
            self.skip = tag
            return
        if self.skip:
            return
        if tag == "meta" and (name in {"description", "robots"} or name.startswith("twitter:") or prop.startswith("og:")):
            return
        if tag == "link" and "canonical" in values.get("rel", "").split():
            return
        self.parts.append(self.get_starttag_text())

    def handle_startendtag(self, tag, attrs):
        self.handle_starttag(tag, attrs)

    def handle_endtag(self, tag):
        if self.skip:
            if tag == self.skip:
                self.skip = None
            return
        self.parts.append(f"</{tag}>")

    def handle_data(self, data):
        if not self.skip:
            self.parts.append(data)

    def handle_entityref(self, name):
        self.handle_data(f"&{name};")

    def handle_charref(self, name):
        self.handle_data(f"&#{name};")

    def handle_comment(self, data):
        if not self.skip:
            self.parts.append(f"<!--{data}-->")

    def handle_decl(self, decl):
        self.parts.append(f"<!{decl}>")


def page_head(head, page):
    parser = PageHead()
    parser.feed(head)
    parser.close()
    title = html.escape(page["title"], quote=True)
    description = html.escape(page["description"], quote=True)
    url = "https://krate.tech/" + page["slug"]
    schema = json.dumps({
        "@context": "https://schema.org", "@type": "WebPage",
        "@id": url + "#webpage", "url": url, "name": page["title"],
        "description": page["description"], "inLanguage": "en",
        "isPartOf": {"@type": "WebSite", "@id": "https://krate.tech/#website", "name": "Krate", "url": "https://krate.tech/"},
    }, ensure_ascii=False).replace("<", "\\u003c")
    metadata = f'''<title>{title}</title>
  <meta name="description" content="{description}">
  <link rel="canonical" href="{url}">
  <meta property="og:type" content="website">
  <meta property="og:site_name" content="Krate">
  <meta property="og:title" content="{title}">
  <meta property="og:description" content="{description}">
  <meta property="og:url" content="{url}">
  <meta property="og:image" content="https://krate.tech/og-v3.png">
  <meta property="og:image:alt" content="Krate desktop application runtime">
  <meta name="twitter:card" content="summary_large_image">
  <meta name="twitter:title" content="{title}">
  <meta name="twitter:description" content="{description}">
  <meta name="twitter:image" content="https://krate.tech/og-v3.png">
  <script type="application/ld+json">{schema}</script>
{ANSWER_CSS}'''
    rendered = re.sub(r"</head\s*>", lambda _: metadata + "</head>", "".join(parser.parts), count=1, flags=re.I)
    return "\n".join(line.rstrip() for line in rendered.splitlines())


def section_id(title):
    return re.sub(r"[^a-z0-9]+", "-", title.lower()).strip("-")


def render(page):
    head, nav, foot = chrome()

    head = page_head(head, page)

    section_ids = [section_id(title) for title, _ in page["sections"]]
    if len(section_ids) != len(set(section_ids)) or not all(section_ids):
        raise ValueError(f"Section anchors must be unique: {page['slug']}")
    actions = "\n".join(
        f'<a class="pill{(" pill-primary" if index == 0 else "")}" '
        f'href="{html.escape(url, quote=True)}">{html.escape(label)}</a>'
        for index, (label, url) in enumerate(page["entry_actions"])
    )
    entry_note = f'<p>{page["entry_note"]}</p>' if page.get("entry_note") else ""
    sections = "\n".join(
        f'''    <section id="{section_id(t)}">
      <h2>{html.escape(t)}</h2>
{b}
    </section>'''
        for t, b in page["sections"]
    )
    sections = sections.replace('<pre class="answer-cmd">',
                                '<pre class="answer-cmd" tabindex="0" aria-label="Command example">')

    return f"""{head}
<body>
{nav}
    <main id="main" class="page-wrap">
    <h1>{html.escape(page["h1"])}</h1>
    <p class="page-lede">{page["lead"]}</p>
    <div class="answer-entry">
      <nav class="answer-actions" aria-label="Choose your next step">
{actions}
      </nav>
{entry_note}
    </div>

{sections}

    <section>
      <h2>Build with Krate</h2>
      <p>Start with the runtime and a project that fits the current APIs. The runtime and CLI are MIT OR Apache-2.0; Studio has a separate license. Check the <a href="https://github.com/incyashraj/krate#license">licensing details</a> and <a href="/docs/limits.html">capability limits</a>.</p>
      <!-- A <p> turned into a flex row made these two pills flex children,
           so they took the line-box height (measured 26px) instead of their
           own padding, while the identical pill in the nav measured 44px.
           A div with a class, so the rule below can reach it and the two
           wrap instead of squeezing on a narrow screen. -->
      <div class="answer-actions">
        <a class="pill pill-primary" href="/docs/quickstart.html">Developer quickstart</a>
        <a class="pill" href="/docs/porting.html">Evaluate your app</a>
        <a class="pill" href="/studio/">Make an app in Studio</a>
      </div>
    </section>
    </main>
{foot}
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
            ("What is inside a .krate file?", """<p>The bundle is a ZIP-based application format, not just a renamed executable. Its core entries are <code>manifest.toml</code> and <code>code.wasm</code>. Bundles can also contain assets, source, SDK material and format-specific metadata. The normal authoring workflow carries editable source with the app.</p>
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
        "h1": "Inspect what an AI-built app can access",
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
        "title": "Krate, Electron and Tauri: desktop distribution models",
        "description": "Compare shared codebases with a shared application artifact: what Electron, Tauri and Krate distribute, runtime requirements, migration work and measurement boundaries.",
        "h1": "One codebase is not the same as one app file",
        "lead": "Electron, Tauri and Krate separate application code from platform details differently. The useful question is what you build, what your users install and which capabilities your app needs.",
        "entry_actions": [("Check your project's fit", "/docs/porting.html"),
                          ("Try Krate", "/docs/quickstart.html")],
        "sections": [
            ("Compare the distribution boundary", """<div class="answer-table-wrap" role="region" aria-label="Distribution comparison" tabindex="0"><table class="answer-table">
<caption>Desktop distribution models, reviewed 19 September 2026</caption>
<thead><tr><th scope="col">Question</th><th scope="col">Electron</th><th scope="col">Tauri</th><th scope="col">Krate</th></tr></thead>
<tbody>
<tr><th scope="row">Application artifact</th><td>Platform-specific packaged application.</td><td>Platform-specific application bundle or installer.</td><td>One .krate application bundle for compatible runtimes.</td></tr>
<tr><th scope="row">UI/runtime model</th><td>Chromium and Node.js in Electron's process model.</td><td>Web frontend in an OS WebView with a Rust backend.</td><td>WebAssembly guest using Krate UI and host interfaces.</td></tr>
<tr><th scope="row">Recipient prerequisite</th><td>The packaged application and its platform requirements.</td><td>The packaged application and platform/WebView requirements.</td><td>A compatible native Krate runtime installed for that system.</td></tr>
<tr><th scope="row">Existing app migration</th><td>Fits browser/Node-based applications.</td><td>Fits web UI with Rust/native integration.</td><td>Port logic and adapt UI/host dependencies to supported Krate APIs.</td></tr>
</tbody></table></div>
<p>Electron's <a href="https://www.electronjs.org/docs/latest/tutorial/distribution-overview">distribution guide</a> covers packaging, signing, publishing and updates; its <a href="https://www.electronjs.org/docs/latest/tutorial/process-model">process model</a> explains Chromium and Node. Tauri documents <a href="https://tauri.app/distribute/">platform-specific distribution</a> and its <a href="https://tauri.app/concept/architecture/">WebView/Rust architecture</a>. Tauri does not bundle Chromium like Electron.</p>"""),
            ("What Krate changes", """<p>With Krate, the developer builds a component against Krate's interfaces and packages the application once. The recipient's native runtime supplies the platform-specific implementation. The same application bytes can be copied between supported desktop systems; the runtime binary itself differs.</p>
<p>This trades per-application platform packaging for dependence on a shared runtime and its API coverage. Runtime delivery, updates, OS trust checks and compatibility still need maintenance. A shared artifact also does not eliminate cross-platform behavior testing.</p>"""),
            ("How to decide for your project", """<p>If your application depends on a browser DOM or Node ecosystem, account for that investment before moving away from Electron. If you want a web UI plus custom Rust/native integrations, examine Tauri's APIs and deployment requirements. Neither choice means rewriting the entire application independently for every OS.</p>
<p>Evaluate Krate when distributing the same application file matters and your required features map to its interfaces. Start with <code>krate port ./my-project</code>, then validate the findings against the <a href="/docs/limits.html">current capability limits</a>. A scan is evidence for planning, not proof the port will preserve every feature.</p>
<p>For a specific app, compare a representative end-to-end task first. If an essential OS integration is missing, stay with a suitable platform or contribute that capability before migrating. See the <a href="/docs/porting.html">porting checklist</a>.</p>"""),
            ("Compare costs at the same boundary", """<p>Do not compare a compressed component with another product's whole installed application and call the ratio a universal win. Record the full download, installed footprint, runtime requirements and additional cost of the next app separately.</p>
<p>For performance, hold the task and inputs constant, record versions and hardware, distinguish cold/warm startup and count all relevant processes. Native dependencies, renderer work and application design can dominate. The <a href="/reports/">Krate reports</a> describe particular workloads; they are not benchmarks of all Electron or Tauri applications.</p>"""),
            ("Try the model before choosing it", """<p>Use the <a href="/docs/quickstart.html">quickstart</a> to open an app, then follow the <a href="/portable-desktop-app-format.html">same-artifact verification procedure</a> across your target systems. Build a representative feature with real data, not just an empty window. Record missing APIs, behavioral differences and deployment friction alongside the benefits.</p>"""),
        ],
    },
]


class MetadataTests(unittest.TestCase):
    def test_homepage_identity_is_replaced_with_varied_html(self):
        variants = [
            '<meta name="description" content="old" />',
            "<meta content='old' NAME='description'>",
            '<META content="old" name="description"/>',
        ]
        for description in variants:
            with self.subTest(description=description):
                original = f'''<!DOCTYPE html><html lang="en"><head>
                <title>OLD HOME</title>{description}
                <link href='https://krate.tech/' rel='canonical'>
                <meta content='OLD HOME' property='og:title'>
                <meta content='OLD HOME' name='twitter:title'>
                <script type='application/ld+json'>{{"name":"OLD HOME"}}</script>
                <style>.kept {{ color: red; }}</style></head>'''
                result = page_head(original, PAGES[0])
                self.assertNotIn("OLD HOME", result)
                self.assertNotIn('content="old"', result)
                self.assertNotIn("content='old'", result)
                self.assertEqual(result.count('rel="canonical"'), 1)
                self.assertEqual(result.count('name="description"'), 1)
                self.assertIn(".kept { color: red; }", result)

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
                self.assertEqual(schema["@type"], "WebPage")
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
                entry = result.index('<nav class="answer-actions" aria-label="Choose your next step">')
                self.assertLess(entry, result.index('<section id="'))
                self.assertEqual(len(page["entry_actions"]), 2)
                for label, href in page["entry_actions"]:
                    self.assertIn(html.escape(label), result)
                    self.assertIn(f'href="{html.escape(href, quote=True)}"', result)
                for href in ("/docs/quickstart.html", "/docs/porting.html", "/docs/limits.html", "/studio/"):
                    self.assertIn(f'href="{href}"', result)
                self.assertIn('href="/docs/quickstart.html#get-krate">Install</a>', result)
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

    def test_duplicate_section_anchors_fail_before_generation(self):
        page = dict(PAGES[0], sections=[("Same title", "<p>One</p>"), ("Same title!", "<p>Two</p>")])
        with self.assertRaisesRegex(ValueError, "Section anchors must be unique"):
            render(page)

    def test_article_shell_defines_its_own_layout_and_accessible_controls(self):
        # These class names do not exist in the homepage stylesheet. A
        # generated article must not depend on unrelated homepage selectors.
        for selector in (".subnav", ".subnav-inner", ".page-wrap",
                         ".page-wrap h1", ".page-wrap h2", ".subfoot", ".subfoot-inner"):
            self.assertIn(selector, ANSWER_CSS)
        self.assertIn("width: min(760px, calc(100% - 48px))", ANSWER_CSS)
        self.assertIn("width: calc(100% - 40px)", ANSWER_CSS)
        self.assertIn("min-height: 44px", ANSWER_CSS)
        self.assertIn(":focus-visible", ANSWER_CSS)
        self.assertIn("scroll-margin-top: 144px", ANSWER_CSS)
        for page in PAGES:
            result = render(page)
            self.assertIn(ANSWER_CSS.strip(), result)
            self.assertIn('<nav aria-label="Primary">', result)
            for opening in re.findall(r'<pre\b[^>]*>', result):
                self.assertIn('tabindex="0"', opening)
                self.assertIn('aria-label="Command example"', opening)

    def test_metadata_is_escaped(self):
        page = dict(PAGES[0], title='A "quoted" <title> & more', description='Keep </script> as text')
        result = page_head("<html><head></head>", page)
        self.assertIn("&quot;quoted&quot; &lt;title&gt; &amp; more", result)
        schemas = re.findall(r'<script type="application/ld\+json">(.*?)</script>', result, re.S)
        self.assertEqual(json.loads(schemas[0])["description"], page["description"])


def main() -> int:
    OUT.mkdir(parents=True, exist_ok=True)
    for page in PAGES:
        target = OUT / page["slug"]
        target.write_text(render(page))
        print(f"  wrote docs/answers/{page['slug']}")
    return 0


if __name__ == "__main__":
    if "--self-test" in sys.argv:
        unittest.main(argv=[sys.argv[0]])
    else:
        raise SystemExit(main())
