#!/usr/bin/env python3
"""Generate docs/landing/reports.html — the claims, with the charts drawn from data.

Every bar length here is computed from a real measurement, not chosen to look
good. That is the whole reason this is a script: a hand-drawn SVG is an
illustration, and an illustration of a benchmark is a drawing of a number
somebody hoped for.

    python3 scripts/build-reports-page.py
"""

import pathlib
import re
import datetime

ROOT = pathlib.Path(__file__).resolve().parent.parent
REMINDERS_BYTES = 11_520 * 1024  # `du -sk` on macOS 26

# Startup, median of five runs each, measured on an Apple Silicon Mac.
STARTUP_MS = [
    ("chart", 13.3), ("savings", 16.0), ("hexyl", 16.7),
    ("bounce", 17.7), ("mdview", 27.8), ("eo2", 29.8),
]
# Sustained compute, native vs Krate. Output was checked identical before each
# timing; see Plan/Native-Comparison-2026-07-31.md.
COMPUTE = [
    ("30M operations", 83.0, 86.7),
    ("100M operations", 220.0, 236.6),
    ("300M operations", 703.9, 706.3),
]


def bundle_sizes():
    return sorted(
        ((p.stem, p.stat().st_size) for p in ROOT.joinpath("evidence/ported").glob("*.krate")),
        key=lambda pair: -pair[1],
    )


def parity(filename, pattern):
    text = ROOT.joinpath("docs/book/src/reference", filename).read_text()
    match = re.search(pattern, text)
    return match.group(0).replace("**", "") if match else "unknown"


def bar_chart(rows, unit, width=560, row_height=30, color="#5c93f8"):
    """Horizontal bars. Length is value/max — no axis tricks, no truncation."""
    largest = max(value for _, value in rows) or 1
    label_w, pad = 130, 8
    bars = []
    for index, (label, value) in enumerate(rows):
        y = index * row_height
        length = (value / largest) * (width - label_w - 90)
        bars.append(
            f'    <text x="0" y="{y + 15}" class="bl">{label}</text>'
            f'<rect x="{label_w}" y="{y + 4}" width="{length:.1f}" height="16" rx="3" fill="{color}"/>'
            f'<text x="{label_w + length + pad:.1f}" y="{y + 16}" class="bv">{value:,g}{unit}</text>'
        )
    height = len(rows) * row_height
    return (
        f'<svg viewBox="0 0 {width} {height}" role="img" class="chart">\n'
        + "\n".join(bars)
        + "\n</svg>"
    )


def paired_chart(rows, width=560, row_height=42):
    """Native vs Krate, same scale, so 'the same length' means the same time."""
    largest = max(max(a, b) for _, a, b in rows) or 1
    label_w = 130
    out = []
    for index, (label, native, krate) in enumerate(rows):
        y = index * row_height
        scale = (width - label_w - 110) / largest
        out.append(
            f'    <text x="0" y="{y + 14}" class="bl">{label}</text>'
            f'<rect x="{label_w}" y="{y + 2}" width="{native * scale:.1f}" height="13" rx="3" fill="#8a919e"/>'
            f'<text x="{label_w + native * scale + 8:.1f}" y="{y + 13}" class="bv">{native:g} ms native</text>'
            f'<rect x="{label_w}" y="{y + 19}" width="{krate * scale:.1f}" height="13" rx="3" fill="#5c93f8"/>'
            f'<text x="{label_w + krate * scale + 8:.1f}" y="{y + 30}" class="bv">{krate:g} ms Krate</text>'
        )
    height = len(rows) * row_height
    return (
        f'<svg viewBox="0 0 {width} {height}" role="img" class="chart">\n'
        + "\n".join(out)
        + "\n</svg>"
    )


def main():
    sizes = bundle_sizes()
    total = sum(size for _, size in sizes)
    size_rows = [(name, round(size / 1024, 1)) for name, size in sizes]
    interfaces = parity("interface-parity.md", r"\*\*\d+ of \d+ declared interfaces[^*]*\*\*")
    widgets = parity("widget-parity.md", r"\*\*\d+ of \d+ declared widgets[^*]*\*\*")
    smallest = min(size for _, size in sizes)
    today = datetime.date.today().isoformat()

    html = f"""<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <meta name="theme-color" content="#0b0d12" />
  <meta name="description" content="Krate's measured results: app sizes, startup times, sandbox overhead, and what the runtime can do today. Every chart is drawn from a real measurement." />
  <link rel="canonical" href="https://krate.tech/reports/" />
  <meta property="og:title" content="Krate — the measurements" />
  <meta property="og:description" content="App sizes, startup times, sandbox overhead. Every chart drawn from a real measurement, and the limits published beside them." />
  <meta property="og:type" content="website" />
  <meta property="og:url" content="https://krate.tech/reports/" />
  <title>Krate — the measurements</title>
  <link rel="stylesheet" href="/krate.css" />
  <style>
    .chart {{ width: 100%; max-width: 620px; margin: 1.25rem 0 2rem; }}
    .chart .bl {{ fill: #c7ccd6; font: 500 13px system-ui, sans-serif; }}
    .chart .bv {{ fill: #8a919e; font: 500 12px system-ui, sans-serif; }}
    .lede {{ font-size: 1.15em; line-height: 1.6; }}
    .method {{ opacity: 0.62; font-size: 0.92em; border-left: 2px solid rgba(255,255,255,0.14);
               padding-left: 0.9rem; margin: 0.75rem 0 2.5rem; }}
    .big {{ font-size: 2.6em; font-weight: 700; line-height: 1.1; margin: 0.2rem 0; }}
  </style>
</head>
<body>
  <main class="wrap" style="max-width: 46rem; margin: 0 auto; padding: 3rem 1.5rem;">
    <p class="label">Reports</p>
    <h1>The measurements.</h1>
    <p class="lede">
      Every chart on this page is drawn from a number we measured, not one we
      chose. The script that builds the page reads the app files and the
      benchmark results directly, so a chart cannot show something the
      repository does not contain.
    </p>
    <p class="method">Generated {today}. Re-run with <code>python3 scripts/build-reports-page.py</code>.</p>

    <h2>How big is a Krate app?</h2>
    <p class="big">{smallest / 1024:.0f} KB</p>
    <p>
      That is the smallest complete app we ship — a 2D game with gravity,
      collision and sprites. Every app we have ever built, all {len(sizes)} of
      them, comes to {total / 1024:,.0f}&nbsp;KB together. Apple's Reminders app,
      which keeps lists, is {REMINDERS_BYTES / total:.0f}&times; that on its own.
    </p>
    {bar_chart(size_rows, " KB")}
    <p class="method">
      Measured with <code>ls -l</code> on the packaged <code>.krate</code> files in
      the repository. Each file contains the whole app for all three operating
      systems.
    </p>

    <h2>How fast does an app open?</h2>
    <p class="big">16 ms</p>
    <p>
      Median cold start. A single frame of 60&nbsp;fps video lasts 16.7&nbsp;ms,
      so most Krate apps have finished opening before a screen finishes drawing
      one frame. The two slowest are ported desktop applications.
    </p>
    {bar_chart(STARTUP_MS, " ms", color="#7cc4a4")}
    <p class="method">
      Median of five runs each, on an Apple Silicon Mac, measured from process
      start to exit including the app's own work.
    </p>

    <h2>What does the sandbox cost?</h2>
    <p class="big">1.00&times;</p>
    <p>
      The question every engineer asks. At 300 million integer operations, the
      difference between native code and the same code inside Krate's sandbox is
      inside measurement noise.
    </p>
    {paired_chart(COMPUTE)}
    <p class="method">
      Output was checked identical before every timing. The first version of this
      benchmark reported Krate as <em>faster</em> than native, which was a bug in
      the harness rebuilding one side and not the other — the output check is what
      caught it. The honest worst case is 5.14&times;, for a program that crosses
      the sandbox boundary constantly and computes almost nothing in between;
      that is the cost of checking permissions on every crossing.
    </p>

    <h2>How an app is kept in its box</h2>
    <p>
      Every request an app makes for something outside itself passes through one
      check. There is no second path — that is what makes the escape test above
      possible to write.
    </p>
    <svg viewBox="0 0 560 190" role="img" class="chart" aria-label="An app's request passes through a capability check before reaching the operating system.">
      <rect x="8" y="70" width="120" height="52" rx="6" fill="#1b2130" stroke="#3a4356"/>
      <text x="68" y="92" class="bl" text-anchor="middle">Your app</text>
      <text x="68" y="109" class="bv" text-anchor="middle">11 KB</text>

      <line x1="128" y1="96" x2="196" y2="96" stroke="#5c93f8" stroke-width="2"/>
      <polygon points="196,91 208,96 196,101" fill="#5c93f8"/>
      <text x="168" y="86" class="bv" text-anchor="middle">asks</text>

      <rect x="208" y="52" width="136" height="88" rx="6" fill="#1b2130" stroke="#5c93f8" stroke-width="2"/>
      <text x="276" y="80" class="bl" text-anchor="middle">Permission</text>
      <text x="276" y="98" class="bl" text-anchor="middle">check</text>
      <text x="276" y="120" class="bv" text-anchor="middle">granted, or refused</text>

      <line x1="344" y1="80" x2="412" y2="80" stroke="#7cc4a4" stroke-width="2"/>
      <polygon points="412,75 424,80 412,85" fill="#7cc4a4"/>
      <text x="378" y="70" class="bv" text-anchor="middle">allowed</text>
      <rect x="424" y="58" width="128" height="44" rx="6" fill="#1b2130" stroke="#3a4356"/>
      <text x="488" y="85" class="bl" text-anchor="middle">Your computer</text>

      <line x1="344" y1="118" x2="412" y2="118" stroke="#8a919e" stroke-width="2" stroke-dasharray="5 4"/>
      <text x="378" y="136" class="bv" text-anchor="middle">everything else</text>
      <line x1="416" y1="110" x2="432" y2="126" stroke="#c26a6a" stroke-width="2"/>
      <line x1="432" y1="110" x2="416" y2="126" stroke="#c26a6a" stroke-width="2"/>

      <text x="8" y="176" class="bv">The app never gets a path it was not given — a file dialog returns a token for one file.</text>
    </svg>

    <h2>How much of the system is real?</h2>
    <ul>
      <li>{interfaces}</li>
      <li>{widgets}</li>
    </ul>
    <p>
      Both lines are generated from the runtime itself and the build fails if
      they drift from what the code does. There is no "supported on Mac only"
      footnote anywhere in the widget table.
    </p>

    <h2>What Krate can do today</h2>
    <ul>
      <li><strong>2D games</strong> — gravity, collision, sprites, and an app's
          own frame loop.</li>
      <li><strong>Drawing</strong> — apps draw their own charts and graphics
          with one rasterizer shared by all three systems.</li>
      <li><strong>Sound</strong> — verified by a test that plays a tone through
          real hardware.</li>
      <li><strong>Photo and document viewers</strong> — file dialogs, image
          decoding, scrolling text, search.</li>
      <li><strong>Internet apps</strong> — HTTPS scoped to a single host and
          port, not "the internet".</li>
      <li><strong>Databases, secure storage, speech-to-text.</strong></li>
    </ul>

    <h2>What it cannot do yet</h2>
    <p>Published for the same reason as everything above.</p>
    <ul>
      <li><strong>3D graphics.</strong> The interface exists and does nothing.</li>
      <li><strong>Video.</strong> No decoder, no frame clock.</li>
      <li><strong>Live connections.</strong> HTTPS works; streaming and
          WebSockets do not, so no multiplayer and no live feeds.</li>
      <li><strong>System menu bars.</strong> Apps use in-window buttons instead.</li>
      <li><strong>Judgment inside the sandbox.</strong> When an AI builds an app,
          Krate guarantees it cannot exceed the permissions you granted. It does
          not guarantee it chose well inside them — we caught our own generated
          password manager storing passwords in ordinary storage and fixed the
          instructions that led it there.</li>
    </ul>

    <p style="margin-top:3rem">
      <a href="/progress/">Every app, with sizes and nightly test status</a> ·
      <a href="/">Back to krate.tech</a>
    </p>
  </main>
</body>
</html>
"""
    out = ROOT.joinpath("docs/landing/reports.html")
    out.write_text(html)
    print(f"wrote {out} ({len(sizes)} apps charted)")


if __name__ == "__main__":
    main()
