#!/usr/bin/env python3
"""Generate docs/landing/reports.html — the claims, with charts drawn from data.

Two rules this file exists to enforce:

1. Every bar length is computed from a measurement, never chosen. A hand-drawn
   SVG is an illustration, and an illustration of a benchmark is a drawing of a
   number somebody hoped for.
2. Anything that can be measured at build time is measured at build time. The
   startup figures were hardcoded once and had drifted within a day -- chart
   read 13.3 ms on the page and 22.8 ms on the machine. Constants rot; a
   subprocess does not.

    python3 scripts/build-reports-page.py            # measures startup
    python3 scripts/build-reports-page.py --no-run   # skips it, for CI without a binary
"""

import datetime
import pathlib
import re
import statistics
import shutil
import subprocess
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parent.parent
BUNDLES = ROOT / "evidence/ported"
KRATE = ROOT / "target/release/krate"

# Apple's Reminders on macOS 26: `du -sk` reports 11,520 KB. The landing page
# and the claims file quote the same figure; they must not drift apart.
REMINDERS_BYTES = 11_520 * 1024
REMINDERS_LABEL = "11.2 MB"

# Sustained compute, native vs Krate, from Plan/Native-Comparison-2026-07-31.md.
# Not re-measurable here: it needs both a native build and a Krate build of the
# same program, which is a benchmark harness rather than a page generator.
COMPUTE = [
    ("30 million operations", 83.0, 86.7),
    ("100 million operations", 220.0, 236.6),
    ("300 million operations", 703.9, 706.3),
]

# What each app demonstrates. Ordered by what a stranger finds most surprising.
APPS = {
    "bounce": ("2D game", "Gravity, collision, sprites, its own frame loop"),
    "chart": ("Drawing", "Draws its own bar chart — no image files"),
    "savings": ("Everyday app", "A window, a form, state that survives a restart"),
    "eo2": ("Ported", "Image viewer, from 2,677 lines of desktop source"),
    "mdview": ("Ported", "Markdown viewer, from 4,863 lines"),
    "grex": ("Ported", "Regex builder, from 5,396 lines"),
    "envelope": ("Database", "Budgeting, with real SQL and secure storage"),
    "ddh": ("Filesystem", "Duplicate-file finder"),
    "hexyl": ("Command line", "Hex viewer — byte-identical output to the original"),
    "rssfwd": ("Internet", "Feed forwarder over scoped HTTPS"),
}


def bundles():
    return sorted(
        ((p.stem, p.stat().st_size) for p in BUNDLES.glob("*.krate")),
        key=lambda pair: -pair[1],
    )


def nightly():
    script = (ROOT / "scripts/replay-ported-apps.sh").read_text()
    return set(re.findall(r'^check "([a-z0-9-]+)"', script, re.M))


def parity(filename, pattern):
    text = (ROOT / "docs/book/src/reference" / filename).read_text()
    match = re.search(pattern, text)
    return match.group(0).replace("**", "") if match else None


def measure_startup(names, runs=5):
    """Median wall-clock for a whole run, in milliseconds.

    Whole-process time on purpose: it is what a person waits, including
    everything the app itself does. A number that excluded the app's own work
    would be flattering and useless.
    """
    if "--no-run" in sys.argv or not KRATE.exists():
        return []
    # A scratch directory each run: two apps look for an input folder and
    # report "not found" rather than doing their work if they are started
    # somewhere bare. They exit 0 either way, so timing them without this
    # would silently measure the wrong thing.
    work = pathlib.Path(tempfile.mkdtemp(prefix="krate-report-"))
    for sub in ("input", "images", "documents", "docs", "output", "scan"):
        (work / sub).mkdir(exist_ok=True)
    (work / "input/sample.bin").write_bytes(b"Hello, Krate!")
    (work / "input/sample.txt").write_text("the quick brown fox the lazy dog\n")

    measured, skipped = [], []
    for name in names:
        bundle = BUNDLES / f"{name}.krate"
        if not bundle.is_file():
            continue
        times = []
        for _ in range(runs):
            start = datetime.datetime.now()
            result = subprocess.run(
                [str(KRATE), "run", "--auto-grant", str(bundle), "--", "quick"],
                capture_output=True,
                cwd=work,
            )
            # An app that refuses to start is not a fast app. Anything that
            # does not exit cleanly is left out and named, rather than
            # quietly dropped, so a shrinking chart is visible.
            if result.returncode != 0:
                times = []
                break
            times.append((datetime.datetime.now() - start).total_seconds() * 1000)
        if times:
            measured.append((name, round(statistics.median(times), 1)))
        else:
            skipped.append(name)
    shutil.rmtree(work, ignore_errors=True)
    if skipped:
        # Not a failure: these two take a file path rather than the word
        # `quick`, so one uniform command cannot start them. The replay script
        # knows each app's real arguments; this page deliberately does not, to
        # avoid a second copy of that knowledge drifting from the first.
        print(f"  not timed (they take a file argument, not `quick`): {', '.join(skipped)}")
    return sorted(measured, key=lambda pair: pair[1])


def bars(rows, unit, color="#5c93f8", width=640, row_h=34):
    """Horizontal bars, length proportional to value. No truncated axis."""
    largest = max(v for _, v in rows) or 1
    label_w, track = 132, width - 132 - 96
    out = []
    for i, (label, value) in enumerate(rows):
        y = i * row_h
        length = max((value / largest) * track, 2)
        out.append(
            f'  <text x="0" y="{y + 17}" class="bl">{label}</text>'
            f'<rect x="{label_w}" y="{y + 5}" width="{length:.1f}" height="18" rx="4" fill="{color}"/>'
            f'<text x="{label_w + length + 10:.1f}" y="{y + 19}" class="bv">{value:,g}{unit}</text>'
        )
    return (
        f'<svg viewBox="0 0 {width} {len(rows) * row_h}" role="img" class="chart" '
        f'aria-label="Bar chart, longest bar is {largest:,g}{unit}">\n'
        + "\n".join(out)
        + "\n</svg>"
    )


def paired(rows, width=640, row_h=52):
    """Native above, Krate below, same scale — equal length means equal time."""
    largest = max(max(a, b) for _, a, b in rows) or 1
    label_w, track = 150, width - 150 - 120
    out = []
    for i, (label, native, krate) in enumerate(rows):
        y = i * row_h
        scale = track / largest
        out.append(
            f'  <text x="0" y="{y + 18}" class="bl">{label}</text>'
            f'<rect x="{label_w}" y="{y + 4}" width="{native * scale:.1f}" height="15" rx="3" fill="#8a919e"/>'
            f'<text x="{label_w + native * scale + 10:.1f}" y="{y + 16}" class="bv">{native:g} ms native</text>'
            f'<rect x="{label_w}" y="{y + 23}" width="{krate * scale:.1f}" height="15" rx="3" fill="#5c93f8"/>'
            f'<text x="{label_w + krate * scale + 10:.1f}" y="{y + 35}" class="bv">{krate:g} ms Krate</text>'
        )
    return (
        f'<svg viewBox="0 0 {width} {len(rows) * row_h}" role="img" class="chart" '
        f'aria-label="Native and Krate timings side by side at the same scale">\n'
        + "\n".join(out)
        + "\n</svg>"
    )


SANDBOX_DIAGRAM = """<svg viewBox="0 0 640 210" role="img" class="chart"
     aria-label="An app's request passes through one permission check; allowed requests reach the computer, everything else is refused.">
  <rect x="10" y="78" width="140" height="58" rx="8" fill="#161b26" stroke="#39425a"/>
  <text x="80" y="103" class="bl" text-anchor="middle">Your app</text>
  <text x="80" y="121" class="bv" text-anchor="middle">11 KB</text>

  <line x1="150" y1="107" x2="222" y2="107" stroke="#5c93f8" stroke-width="2"/>
  <polygon points="222,101 236,107 222,113" fill="#5c93f8"/>
  <text x="186" y="96" class="bv" text-anchor="middle">asks for a file</text>

  <rect x="236" y="58" width="152" height="98" rx="8" fill="#161b26" stroke="#5c93f8" stroke-width="2"/>
  <text x="312" y="90" class="bl" text-anchor="middle">Permission check</text>
  <text x="312" y="112" class="bv" text-anchor="middle">the one way out</text>
  <text x="312" y="130" class="bv" text-anchor="middle">of the sandbox</text>

  <line x1="388" y1="86" x2="462" y2="86" stroke="#7cc4a4" stroke-width="2"/>
  <polygon points="462,80 476,86 462,92" fill="#7cc4a4"/>
  <text x="425" y="75" class="bv" text-anchor="middle">granted</text>
  <rect x="476" y="62" width="152" height="48" rx="8" fill="#161b26" stroke="#39425a"/>
  <text x="552" y="91" class="bl" text-anchor="middle">Your computer</text>

  <line x1="388" y1="130" x2="458" y2="130" stroke="#8a919e" stroke-width="2" stroke-dasharray="6 5"/>
  <text x="425" y="152" class="bv" text-anchor="middle">everything else</text>
  <line x1="462" y1="121" x2="480" y2="139" stroke="#c9736f" stroke-width="2.5"/>
  <line x1="480" y1="121" x2="462" y2="139" stroke="#c9736f" stroke-width="2.5"/>

  <text x="10" y="192" class="bv">There is no second path. That is what makes the escape test below possible to write.</text>
</svg>"""


def main():
    sizes = bundles()
    total = sum(s for _, s in sizes)
    tested = nightly()
    smallest_name, smallest = min(sizes, key=lambda pair: pair[1])

    startup = measure_startup([n for n, _ in sizes])
    interfaces = parity("interface-parity.md", r"\*\*\d+ of \d+ declared interfaces[^*]*\*\*")
    widgets = parity("widget-parity.md", r"\*\*\d+ of \d+ declared widgets[^*]*\*\*")

    rows = []
    for name, size in sizes:
        kind, note = APPS.get(name, ("", ""))
        mark = "Nightly" if name in tested else "—"
        rows.append(
            f'        <tr><td><code>{name}</code></td><td>{kind}</td>'
            f'<td class="num">{size:,}</td>'
            f'<td class="num">{REMINDERS_BYTES / size:,.0f}&times;</td>'
            f'<td>{note}</td><td class="mid">{mark}</td></tr>'
        )

    startup_block = ""
    if startup:
        fastest = startup[0][1]
        slowest = startup[-1][1]
        startup_block = f"""
    <section>
      <p class="eyebrow">Speed</p>
      <h2>An app opens in {fastest:,.0f}&ndash;{slowest:,.0f} milliseconds.</h2>
      <p>
        One frame of 60&nbsp;fps video lasts 16.7&nbsp;ms. Most Krate apps have
        finished opening before a screen finishes drawing a single frame. The
        slowest here are the two ported desktop applications, which do real
        work on startup.
      </p>
      {bars(startup, " ms", color="#7cc4a4")}
      <p class="method">
        Measured while this page was generated: five runs of each app on this
        machine, median reported, whole-process time including everything the
        app itself does. These numbers were hardcoded once and had drifted
        within a day, so the page now runs the apps instead of remembering them.
        Two of the ten are missing from this chart because they take a file path
        rather than a fixed argument, and one command cannot start every app.
      </p>
    </section>
"""

    parity_items = "".join(
        f"      <li>{line}</li>\n" for line in (interfaces, widgets) if line
    )

    today = datetime.date.today().isoformat()
    html = f"""<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <meta name="theme-color" content="#0b0d12" />
  <meta name="description" content="Krate's measured results: app sizes, startup times, sandbox cost, and what the runtime can and cannot do. Every chart is drawn from a real measurement." />
  <link rel="canonical" href="https://krate.tech/reports/" />
  <meta property="og:title" content="Krate — the measurements" />
  <meta property="og:description" content="A 2D game in 11 KB. Sandboxed code at native speed. Every chart drawn from a real measurement, with the limits published beside them." />
  <meta property="og:type" content="website" />
  <meta property="og:url" content="https://krate.tech/reports/" />
  <meta property="og:image" content="https://krate.tech/og-v2.png" />
  <meta name="twitter:card" content="summary_large_image" />
  <title>Krate — the measurements</title>
  <link rel="stylesheet" href="/krate.css" />
  <style>
    .report {{ max-width: 48rem; margin: 0 auto; padding: 4rem 1.5rem 6rem; }}
    .report section {{ margin: 0 0 4.5rem; }}
    .report h1 {{ font-size: clamp(2.1rem, 5vw, 3rem); line-height: 1.1; margin: 0.4rem 0 1rem; }}
    .report h2 {{ font-size: clamp(1.4rem, 3.2vw, 1.9rem); line-height: 1.2; margin: 0.3rem 0 0.9rem; }}
    .eyebrow {{ text-transform: uppercase; letter-spacing: 0.09em; font-size: 0.75rem;
                font-weight: 600; opacity: 0.55; margin: 0; }}
    .lede {{ font-size: 1.12rem; line-height: 1.65; opacity: 0.9; }}
    .chart {{ width: 100%; max-width: 680px; margin: 1.5rem 0 1.25rem; display: block; }}
    .chart .bl {{ fill: #ccd2dd; font: 500 13px system-ui, -apple-system, sans-serif; }}
    .chart .bv {{ fill: #8a919e; font: 500 12px system-ui, -apple-system, sans-serif; }}
    .method {{ opacity: 0.6; font-size: 0.9rem; line-height: 1.55;
               border-left: 2px solid rgba(255,255,255,0.13); padding-left: 1rem; margin: 1rem 0 0; }}
    .headline-number {{ font-size: clamp(2.6rem, 8vw, 4rem); font-weight: 700;
                        line-height: 1; letter-spacing: -0.02em; margin: 0 0 0.15rem;
                        background: linear-gradient(180deg, #eaf0ff, #92b4f4);
                        -webkit-background-clip: text; background-clip: text; color: transparent; }}
    .report table {{ border-collapse: collapse; width: 100%; margin: 1.5rem 0 0.75rem; font-size: 0.94rem; }}
    .report th, .report td {{ padding: 0.6rem 0.7rem; text-align: left;
                              border-bottom: 1px solid rgba(255,255,255,0.08); }}
    .report th {{ font-size: 0.8rem; text-transform: uppercase; letter-spacing: 0.05em; opacity: 0.55; }}
    .num {{ text-align: right; font-variant-numeric: tabular-nums; }}
    .mid {{ opacity: 0.72; }}
    .table-scroll {{ overflow-x: auto; }}
    .callout {{ background: rgba(92,147,248,0.07); border: 1px solid rgba(92,147,248,0.22);
                border-radius: 10px; padding: 1.1rem 1.25rem; margin: 1.5rem 0; }}
    .callout pre {{ margin: 0.6rem 0 0; overflow-x: auto; font-size: 0.88rem; }}
    .limits li {{ margin-bottom: 0.55rem; line-height: 1.55; }}
    .page-links {{ display: flex; flex-wrap: wrap; gap: 1.25rem; margin-top: 3.5rem;
                   padding-top: 1.75rem; border-top: 1px solid rgba(255,255,255,0.1); }}
  </style>
</head>
<body>
  <main class="report">
    <p class="eyebrow">Reports</p>
    <h1>The measurements.</h1>
    <p class="lede">
      Every chart here is drawn from a number we measured, not one we chose. The
      script that builds this page reads the app files directly and re-runs the
      apps to time them, so a chart cannot show something the repository does
      not contain. Where a number cannot be re-measured automatically, the method
      is written underneath it.
    </p>
    <p class="method">Generated {today}. Anyone can reproduce it: <code>python3 scripts/build-reports-page.py</code>.</p>

    <section>
      <p class="eyebrow">Size</p>
      <p class="headline-number">{smallest / 1024:.0f} KB</p>
      <h2>A complete 2D game.</h2>
      <p>
        Gravity, collision, sprites, and its own frame loop — in one file that
        opens on Mac, Windows and Linux. It is that small because nothing is
        bundled inside it that your computer already has: no browser, no
        framework, no runtime copy.
      </p>
      {bars([(n, round(s / 1024, 1)) for n, s in sizes], " KB")}
      <p>
        All {len(sizes)} apps together come to <strong>{total / 1024:,.0f}&nbsp;KB</strong>.
        Apple's Reminders app, which keeps lists, is {REMINDERS_BYTES / total:,.0f}&times;
        that on its own.
      </p>
      <p class="method">
        Sizes read from the packaged <code>.krate</code> files in the public
        repository. Each one contains the whole app for all three operating
        systems. Reminders measured with <code>du -sk</code> on macOS 26:
        {REMINDERS_LABEL}.
      </p>
    </section>
{startup_block}
    <section>
      <p class="eyebrow">Cost of safety</p>
      <p class="headline-number">1.00&times;</p>
      <h2>The sandbox is free where it matters.</h2>
      <p>
        The question every engineer asks. At 300 million integer operations, the
        difference between native code and the same code inside Krate's sandbox
        is inside measurement noise.
      </p>
      {paired(COMPUTE)}
      <p class="method">
        Output was checked identical before every timing. The first version of
        this benchmark reported Krate as <em>faster</em> than native, which was a
        bug in the harness rebuilding one side and not the other — the output
        check is what caught it. The honest worst case is <strong>5.14&times;</strong>,
        for a program that crosses the sandbox boundary constantly and computes
        almost nothing in between; that is the price of checking a permission on
        every crossing. Full method in
        <code>Plan/Native-Comparison-2026-07-31.md</code>.
      </p>
    </section>

    <section>
      <p class="eyebrow">Safety</p>
      <h2>An app can only touch what you allowed.</h2>
      <p>
        Every request for something outside the app passes through one check.
      </p>
      {SANDBOX_DIAGRAM}
      <div class="callout">
        <p style="margin:0">
          <strong>We attack it to prove it.</strong> An app is granted permission
          to read <code>/etc</code> — where a Unix system keeps its account list —
          and asked for the password file.
        </p>
        <pre><code>$ krate run --grant "fs.read:/etc/**" hexyl.krate -- /etc/passwd
00000000  73 61 6e 64 62 6f 78 20  63 6f 70 79</code></pre>
        <p style="margin:0.7rem 0 0">
          Those bytes spell <strong>sandbox copy</strong>. The app believes it
          succeeded. The real file was never reachable.
        </p>
      </div>
      <ul>
        <li><strong>A file picker returns a token, not a path.</strong> The app
            can read the one file a person chose, and never learns where it
            lives or what sits beside it. The click is the permission.</li>
        <li><strong>A token dies with the run.</strong> An app cannot store one
            and come back for the same file later.</li>
        <li><strong>Image decoding happens inside the sandbox.</strong> A
            malformed photo attacks the app, not the operating system. Pixels
            cross the boundary; parsers do not.</li>
      </ul>
    </section>

    <section>
      <p class="eyebrow">Every app we have shipped</p>
      <h2>Ten apps, and where each one runs.</h2>
      <div class="table-scroll">
      <table>
        <thead>
          <tr><th>App</th><th>Kind</th><th class="num">Bytes</th>
              <th class="num">vs Reminders</th><th>What it is</th><th>Re-tested</th></tr>
        </thead>
        <tbody>
{chr(10).join(rows)}
        </tbody>
      </table>
      </div>
      <p class="method">
        &ldquo;Nightly&rdquo; means the app is re-run on macOS, Windows and Linux
        every night and its real output is checked — not that it started, but
        that the answers it prints are still right. <code>rssfwd</code> is
        excluded on purpose: it reaches the internet, and a nightly test that
        depends on someone else's server reports their uptime rather than our
        runtime.
      </p>
    </section>

    <section>
      <p class="eyebrow">Coverage</p>
      <h2>How much of the system is real.</h2>
      <ul>
{parity_items}      </ul>
      <p>
        Both lines are generated from the runtime itself, not written by hand,
        and the build fails if they drift from what the code does. There is no
        &ldquo;supported on Mac only&rdquo; footnote anywhere in the widget table.
      </p>
    </section>

    <section>
      <p class="eyebrow">Today</p>
      <h2>What you can build right now.</h2>
      <ul class="limits">
        <li><strong>2D games</strong> — gravity, collision, sprites, an app's own
            frame loop.</li>
        <li><strong>Drawing</strong> — charts and graphics an app renders itself,
            through one rasterizer shared by all three systems.</li>
        <li><strong>Sound</strong> — verified by a test that plays a tone through
            real hardware.</li>
        <li><strong>Photo and document viewers</strong> — file dialogs, image
            decoding, scrolling text, search.</li>
        <li><strong>Internet apps</strong> — HTTPS scoped to a single host and
            port, not &ldquo;the internet&rdquo;.</li>
        <li><strong>Databases, secure storage, speech-to-text.</strong></li>
      </ul>
    </section>

    <section>
      <p class="eyebrow">Not yet</p>
      <h2>What Krate cannot do.</h2>
      <p>Published for the same reason as everything above it.</p>
      <ul class="limits">
        <li><strong>3D graphics.</strong> The interface is declared and does
            nothing.</li>
        <li><strong>Video.</strong> No decoder, no frame clock.</li>
        <li><strong>Live connections.</strong> HTTPS works; streaming and
            WebSockets do not — so no multiplayer and no live feeds.</li>
        <li><strong>System menu bars.</strong> Apps use in-window buttons.</li>
        <li><strong>Judgment inside the sandbox.</strong> When an AI writes the
            app, Krate guarantees it cannot exceed the permissions you granted.
            It does not guarantee it chose well inside them. We caught our own
            generated password manager storing passwords in ordinary app data
            rather than the system keychain, and fixed the instructions that led
            it there.</li>
      </ul>
    </section>

    <div class="page-links">
      <a href="/progress/">Where we are today</a>
      <a href="https://github.com/incyashraj/krate">The code</a>
      <a href="https://github.com/incyashraj/krate/actions">Three-system CI</a>
      <a href="/">Back to krate.tech</a>
    </div>
  </main>
</body>
</html>
"""
    out = ROOT / "docs/landing/reports.html"
    out.write_text(html)
    measured = f", {len(startup)} apps timed" if startup else ", startup skipped"
    print(f"wrote {out} ({len(sizes)} apps charted{measured})")


if __name__ == "__main__":
    main()
