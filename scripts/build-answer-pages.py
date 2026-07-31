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
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LANDING = ROOT / "docs" / "landing" / "index.html"
OUT = ROOT / "docs" / "answers"


def chrome():
    """Head, nav, and footer lifted from the landing page.

    Taken from the real page rather than copied into this script, so these pages
    cannot fall behind a change to the site's own header.
    """
    s = LANDING.read_text()
    head = s[: s.find("</head>") + len("</head>")]
    # Drop the landing page's own structured data; each answer page has its own.
    head = re.sub(r"  <!-- Structured data.*?</script>\n", "", head, flags=re.S)
    # These live one directory down.
    head = head.replace('href="./krate.css"', 'href="/krate.css"')
    head = head.replace('href="./', 'href="/').replace('src="./', 'src="/')

    ns = s.find('<nav class="site-nav"')
    nav = s[ns : s.find("</nav>", ns) + len("</nav>")]
    nav = nav.replace('href="#', 'href="/#').replace('href="./', 'href="/')

    fs = s.find("<footer")
    foot = s[fs : s.find("</footer>", fs) + len("</footer>")] if fs > 0 else ""
    foot = foot.replace('href="#', 'href="/#').replace('href="./', 'href="/')
    return head, nav, foot


def faq_schema(pairs):
    """Structured data so a search result can carry the answer, not just a link."""
    return json.dumps(
        {
            "@context": "https://schema.org",
            "@type": "FAQPage",
            "mainEntity": [
                {
                    "@type": "Question",
                    "name": q,
                    "acceptedAnswer": {"@type": "Answer", "text": a},
                }
                for q, a in pairs
            ],
        },
        indent=2,
    )


def render(page):
    head, nav, foot = chrome()

    head = head.replace(
        "<title>Krate — The cloud for AI-made software | One app file for Mac, Windows, Linux</title>",
        f"<title>{html.escape(page['title'])}</title>",
    )
    head = re.sub(
        r'<meta name="description" content="[^"]*" />',
        f'<meta name="description" content="{html.escape(page["description"])}" />',
        head,
        count=1,
    )
    head = head.replace(
        '<link rel="canonical" href="https://krate.tech/" />',
        f'<link rel="canonical" href="https://krate.tech/{page["slug"]}" />',
    )
    head = head.replace(
        "</head>",
        f'  <script type="application/ld+json">\n{faq_schema(page["faq"])}\n  </script>\n</head>',
    )

    sections = "\n".join(
        f'''    <section class="section">
      <div class="wrap plain-block">
        <h2>{html.escape(t)}</h2>
{b}
      </div>
    </section>'''
        for t, b in page["sections"]
    )

    return f"""{head}
<body>
{nav}
    <main id="main">
    <section class="section">
      <div class="wrap plain-block">
        <h1 class="answer-title">{html.escape(page["h1"])}</h1>
        <p class="answer-lead">{page["lead"]}</p>
      </div>
    </section>

{sections}

    <section class="section">
      <div class="wrap plain-block">
        <h2>Try it</h2>
        <p>Krate is open source and installs with one command. Nothing to sign up for.</p>
        <div class="actions">
          <a class="button primary" href="/#start">Get Krate</a>
          <a class="button" href="/cloud/">Open the store</a>
        </div>
      </div>
    </section>
    </main>
{foot}
</body>
</html>
"""


PAGES = [
    {
        "slug": "share-an-app-made-with-ai.html",
        "title": "How to share an app you made with AI — Krate",
        "description": "You asked AI to build something and it works on your machine. Here is how to send it to someone else so it opens on their Mac, Windows, or Linux computer without a build step.",
        "h1": "How to share an app you made with AI",
        "lead": "It works on your machine. Getting it onto someone else's is the part nobody solved &mdash; until the app becomes one file.",
        "faq": [
            (
                "How do I share an app I made with AI?",
                "Package it into a single .krate file with `krate create`, then send that file however you send any file. The person who receives it installs Krate once, double-clicks the file, and the app opens on Mac, Windows, or Linux from that same file.",
            ),
            (
                "Do I need to build it separately for each operating system?",
                "No. A .krate file contains a WebAssembly component that calls Krate interfaces rather than operating system APIs, so one file runs on all three. There is no per-platform build and no installer to sign.",
            ),
            (
                "Does the person receiving it need to trust me?",
                "Less than you might think. Before the app runs, Krate shows them exactly what it is asking for, in plain words, and gives it nothing else. An app that asks to save a list cannot read the rest of their computer.",
            ),
        ],
        "sections": [
            (
                "The problem with sharing what AI builds",
                """        <p>AI is good at writing a small useful app. The trouble starts after that. A script needs the right language installed. A web app needs hosting and cannot touch local files. A desktop app needs a separate build for each operating system, and on Mac it needs signing before anyone can open it without a warning.</p>
        <p>So the app that took ten minutes to write takes an afternoon to give away, and usually does not get given away at all.</p>""",
            ),
            (
                "Make it one file",
                """        <p>Describe the app, and Krate has AI write it, checks it, and packages the result:</p>
        <pre class="answer-cmd">krate create "a checklist app that saves locally" --agent claude</pre>
        <p>What comes out is <code>checklist.krate</code> &mdash; around 12 KB, containing the app and the list of what it needs to be allowed to do.</p>""",
            ),
            (
                "Send it like a document",
                """        <p>Email it, drop it in a shared folder, put it in a chat. There is nothing else to install alongside it and no link that stops working.</p>
        <p>On the other end they install Krate once with a single command, then double-click the file. It opens on Mac, Windows, and Linux from that same file &mdash; not three downloads.</p>""",
            ),
            (
                "They see what it wants before it runs",
                """        <p>This is what makes sending AI-written software reasonable rather than reckless. Before any of the app's code runs, Krate shows what it is asking for:</p>
        <pre class="answer-cmd">This app is asking to:
  [1] read files in notes (fs.read:notes/**)
      Load your saved notes
  [2] save files in notes (fs.write:notes/**)
      Save the note you are editing

Grant [A]ll / [N]one / numbers (for example 1,2):</pre>
        <p>They decide. The app gets what they allow and nothing more, and if they refuse something it needs, it does not start half-working &mdash; it does not start at all.</p>""",
            ),
        ],
    },
    {
        "slug": "run-ai-generated-code-safely.html",
        "title": "How to run AI-generated code safely — Krate",
        "description": "AI wrote it and you have not read all of it. Krate runs the app with no access to your files or network until you allow each thing, and refuses to package an app that reaches outside what it declared.",
        "h1": "How to run AI-generated code safely",
        "lead": "You did not read every line, and honestly you were not going to. The question is what the app can reach if it turns out to be wrong.",
        "faq": [
            (
                "Is it safe to run code that AI wrote?",
                "Not by default, anywhere. Krate changes what happens when it is wrong: an app starts with no access to your files or the network, must ask for each capability it needs, and you see that list before any of its code runs. It receives only what you allow.",
            ),
            (
                "How do I know what an AI-generated app will do?",
                "Run `krate run app.krate --dump-caps`. It lists every capability the app declared without executing any of it, so you can decide whether to run it at all.",
            ),
            (
                "What stops an app from just ignoring the permission screen?",
                "The check is not inside the app. A Krate app cannot call the operating system directly; it calls Krate interfaces, and the runtime verifies the capability before it touches the host. An app that imports anything outside that boundary is rejected at packaging time and never becomes a .krate file.",
            ),
        ],
        "sections": [
            (
                "The real risk is not bad code, it is reach",
                """        <p>Most AI-generated code is not malicious. It is confidently wrong. It deletes the wrong directory, uploads something it should not have, or loops until the disk fills.</p>
        <p>Reading it all is not realistic once it is more than a page long. The useful question is not whether the code is correct, but what it can touch when it is not.</p>""",
            ),
            (
                "Look at it before you run it",
                """        <p>You can inspect a Krate app without executing a single instruction:</p>
        <pre class="answer-cmd">$ krate run notes.krate --dump-caps

Identity
  - 467e4b0e1124b7a8aa86fb1ce39909046508819d55f161e408e282de177b0f16

This app will ask for
  - read files in notes (fs.read:notes/**)
  - save files in notes (fs.write:notes/**)
  - read from the clipboard (ui.clipboard:read)
  - copy to the clipboard (ui.clipboard:write)</pre>
        <p>That is the real output. The identity is computed from the file's contents, so you can check that what you received is what somebody else verified.</p>""",
            ),
            (
                "Refusing actually refuses",
                """        <p>Withhold something the app needs and it does not start. It does not run in a degraded mode and quietly fail later:</p>
        <pre class="answer-cmd">$ krate run notes.krate --grant ui.window:create

This app needs permission it was not given, so it did not run.
It needs to:
  - read files in notes (fs.read:notes/**)
  - save files in notes (fs.write:notes/**)</pre>
        <p>The check happens before the host is touched, not inside the app where a bug could skip it.</p>""",
            ),
            (
                "Why an app cannot lie about this",
                """        <p>A Krate app is a WebAssembly component. It has no way to call the operating system directly &mdash; only Krate's own interfaces, and every one of those verifies the capability first.</p>
                <p>An app that imports anything outside that boundary is rejected while it is being packaged, so it never becomes a shareable file at all. That check runs whether the code came from a person or a model.</p>""",
            ),
            (
                "What it does not claim",
                """        <p>Krate limits what an app can reach and enforces your decision. It does not prove the app is correct, and it is not yet a safe way to run arbitrary untrusted software from the internet.</p>
        <p>Saying so matters: a security claim that overstates itself is worse than a smaller true one.</p>""",
            ),
        ],
    },
    {
        "slug": "portable-desktop-app-format.html",
        "title": "A portable desktop app format: one file for Mac, Windows, and Linux — Krate",
        "description": "One file that opens on all three desktop operating systems, without a separate build, an installer, or a bundled browser engine. How the .krate format works and where it does not fit.",
        "h1": "One app file for Mac, Windows, and Linux",
        "lead": "Not three installers behind one download button. One file, and the same file, on every desktop.",
        "faq": [
            (
                "Can one file really run on Mac, Windows, and Linux?",
                "Yes. A .krate file contains a WebAssembly component that calls Krate interfaces instead of operating system APIs. The Krate runtime on each system translates approved calls into local behaviour, so the same bytes run everywhere.",
            ),
            (
                "How is this different from Electron?",
                "An Electron app bundles a browser engine, so every app carries about 100 MB of Chromium and still needs platform packaging. A Krate app is typically tens of kilobytes and shares one runtime, and the runtime controls what it can access.",
            ),
            (
                "What kinds of apps fit this format?",
                "Small and medium desktop apps: lists, notes, trackers, file tools, API clients, dashboards. They can open a window, keep settings, keep a database, stay signed in, send notifications, and open links. Games with native engines and system tools are out of scope.",
            ),
        ],
        "sections": [
            (
                "Why one desktop app usually means three",
                """        <p>A desktop app is normally written against one operating system's own interface &mdash; AppKit, Win32, GTK. Supporting all three means three builds, three sets of platform bugs, an installer each, and code signing on at least two.</p>
        <p>That cost is why most small useful software never leaves the machine it was written on.</p>""",
            ),
            (
                "What a .krate file is",
                """        <p>A ZIP archive with two things in it: a WebAssembly component, and a manifest declaring what the app wants to be allowed to do.</p>
        <p>The component never calls the operating system. It calls Krate interfaces described in WIT, and the runtime on each system turns approved calls into local behaviour. The file is the same bytes on every machine, which is checkable: it carries an identity computed from its own contents.</p>""",
            ),
            (
                "What an app can actually do",
                """        <p>Enough for a real app. It can open a window with real controls, keep its own settings, keep its own database, stay signed in, send notifications, open links in your browser, read and write folders you choose, and reach hosts you name.</p>
        <p>Each of those is a separate permission the person sees before the app runs, so an app that keeps a list never sees your folders.</p>""",
            ),
            (
                "Where it does not fit",
                """        <p>This format is for small and medium desktop software. A game with a native engine, a driver, or a tool that needs deep system access is the wrong shape for it, and Krate says so rather than half-supporting them.</p>
        <p><code>krate port</code> reads an existing project without building or running it and tells you which of the three answers applies: ready, needs changes, or not supported yet.</p>""",
            ),
        ],
    },
]


def main() -> int:
    OUT.mkdir(parents=True, exist_ok=True)
    for page in PAGES:
        target = OUT / page["slug"]
        target.write_text(render(page))
        print(f"  wrote docs/answers/{page['slug']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
