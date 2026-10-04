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
.prose pre.answer-cmd { max-width: 100%; overflow-x: auto; white-space: pre; overscroll-behavior-x: contain; -webkit-overflow-scrolling: touch; }
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


def page_head(page):
    """The head of one answer page: its own title, description, canonical,
    social cards and structured data, then the kit."""
    title = html.escape(page["title"], quote=True)
    description = html.escape(page["description"], quote=True)
    url = "https://krate.tech/" + page["slug"]
    schema = json.dumps({
        "@context": "https://schema.org", "@type": "WebPage",
        "@id": url + "#webpage", "url": url, "name": page["title"],
        "description": page["description"], "inLanguage": "en",
        "isPartOf": {"@type": "WebSite", "@id": "https://krate.tech/#website", "name": "Krate", "url": "https://krate.tech/"},
    }, ensure_ascii=False).replace("<", "\\u003c")
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

    section_ids = [section_id(title) for title, _ in page["sections"]]
    if len(section_ids) != len(set(section_ids)) or not all(section_ids):
        raise ValueError(f"Section anchors must be unique: {page['slug']}")
    actions = "\n".join(
        (f'    <a class="btn dark" href="{html.escape(url, quote=True)}">{html.escape(label)} {ARROW}</a>' if index == 0
         else f'    <a class="btn ghost" href="{html.escape(url, quote=True)}">{html.escape(label)}</a>')
        for index, (label, url) in enumerate(page["entry_actions"])
    )
    entry_note = f'  <p class="ent">{page["entry_note"]}</p>\n' if page.get("entry_note") else ""
    toc = "\n".join(f'      <a href="#{section_id(t)}">{html.escape(t)}</a>' for t, _ in page["sections"])
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
  <p class="meta">{read_minutes(page)} min read</p>
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
    </article>
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
        # content needs. A command block scrolls in its own box (at 390px
        # one long line once made every answer page scroll sideways).
        self.assertIn(".prose pre.answer-cmd", ANSWER_CSS)
        self.assertIn("overflow-x: auto", ANSWER_CSS)
        self.assertIn("white-space: pre;", ANSWER_CSS)
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

    def test_metadata_is_escaped(self):
        page = dict(PAGES[0], title='A "quoted" <title> & more', description='Keep </script> as text')
        result = page_head(page)
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
