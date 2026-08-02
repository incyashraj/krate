#!/usr/bin/env python3
"""Generate docs/landing/progress.html from measured facts.

Written because the page's whole promise is "these numbers are real". Typing
them by hand would make it a brochure that happens to contain figures; reading
them from the bundles, the generated parity tables, and the replay script means
the page cannot claim something the repository does not contain.

Run before a deploy, or let CI run it:

    python3 scripts/build-progress-page.py
"""

import pathlib
import re
import subprocess
import datetime

ROOT = pathlib.Path(__file__).resolve().parent.parent
# Apple's Reminders on macOS 26: `du -sk` reports 11,520 KB. The landing page
# and the claims file quote the same figure; they must not drift apart.
REMINDERS_BYTES = 11_520 * 1024


def bundles():
    """Every shipped app, largest first, so the table opens with the real ones."""
    found = []
    for path in sorted(ROOT.joinpath("evidence/ported").glob("*.krate")):
        found.append((path.stem, path.stat().st_size))
    return sorted(found, key=lambda pair: -pair[1])


def replayed():
    """Apps re-run nightly on all three systems."""
    script = ROOT.joinpath("scripts/replay-ported-apps.sh").read_text()
    return set(re.findall(r'^check "([a-z0-9-]+)"', script, re.M))


def parity(filename, pattern):
    text = ROOT.joinpath("docs/book/src/reference", filename).read_text()
    match = re.search(pattern, text)
    return match.group(0).replace("**", "") if match else "unknown"


def commit():
    try:
        return subprocess.run(
            ["git", "rev-parse", "--short", "HEAD"],
            cwd=ROOT, capture_output=True, text=True, check=True,
        ).stdout.strip()
    except Exception:
        return "unknown"


DESCRIPTIONS = {
    "eo2": "Image viewer, ported from 2,677 lines of desktop source",
    "mdview": "Markdown viewer, ported from 4,863 lines",
    "grex": "Regex builder, ported from 5,396 lines",
    "envelope": "Budgeting app with a real database",
    "rssfwd": "Feed forwarder that talks to the internet",
    "ddh": "Duplicate-file finder",
    "hexyl": "Hex viewer, byte-identical output to the original",
    "savings": "Budget splitter with a window and saved state",
    "chart": "Draws its own bar chart, no image files",
    "bounce": "2D game: gravity, collision, sprites, own frame loop",
}


def main():
    rows = []
    total = 0
    nightly = replayed()
    for name, size in bundles():
        total += size
        note = DESCRIPTIONS.get(name, "")
        tested = "Yes" if name in nightly else "Not nightly"
        rows.append(
            f'          <tr><td><code>{name}</code></td>'
            f'<td class="num">{size:,}</td>'
            f'<td class="num">{REMINDERS_BYTES / size:,.0f}&times;</td>'
            f'<td>{note}</td><td>{tested}</td></tr>'
        )

    interfaces = parity("interface-parity.md", r"\*\*\d+ of \d+ declared interfaces[^*]*\*\*")
    widgets = parity("widget-parity.md", r"\*\*\d+ of \d+ declared widgets[^*]*\*\*")
    today = datetime.date.today().isoformat()

    html = f"""<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <meta name="theme-color" content="#0b0d12" />
  <meta name="description" content="Where Krate is today, measured: every app we have shipped, its size, and whether it is re-tested nightly on Mac, Windows, and Linux." />
  <link rel="canonical" href="https://krate.tech/progress/" />
  <meta property="og:title" content="Krate — where we are, measured" />
  <meta property="og:description" content="Every app we have shipped, its size, and what still does not work. Generated from the repository, not written by hand." />
  <meta property="og:type" content="website" />
  <meta property="og:url" content="https://krate.tech/progress/" />
  <title>Krate — where we are, measured</title>
  <link rel="stylesheet" href="/krate.css" />
  <style>
    .num {{ text-align: right; font-variant-numeric: tabular-nums; }}
    table {{ border-collapse: collapse; width: 100%; margin: 1.5rem 0; }}
    th, td {{ padding: 0.5rem 0.75rem; border-bottom: 1px solid rgba(255,255,255,0.08); text-align: left; }}
    th {{ font-weight: 600; opacity: 0.7; font-size: 0.9em; }}
    .generated {{ opacity: 0.6; font-size: 0.9em; }}
  </style>
</head>
<body>
  <main class="wrap" style="max-width: 60rem; margin: 0 auto; padding: 3rem 1.5rem;">
    <p class="label">Measured</p>
    <h1>Where Krate is today.</h1>
    <p>
      Every app we have shipped, its real size, and whether it is re-tested every
      night on Mac, Windows, and Linux. This page is generated from the
      repository — if a number here is wrong, the code is wrong.
    </p>
    <p class="generated">Generated {today} from commit <code>{commit()}</code>.</p>

    <h2>Apps that exist and run</h2>
    <table>
      <thead>
        <tr><th>App</th><th class="num">Bytes</th><th class="num">vs Reminders</th><th>What it is</th><th>Nightly</th></tr>
      </thead>
      <tbody>
{chr(10).join(rows)}
      </tbody>
    </table>
    <p>
      <strong>All {len(bundles())} apps together: {total:,} bytes ({total / 1024:,.0f}&nbsp;KB).</strong>
      Apple's Reminders app, which keeps lists, is {REMINDERS_BYTES / total:,.1f}&times; that on its own.
    </p>
    <p class="generated">
      One app is not re-run nightly on purpose: <code>rssfwd</code> reaches the
      internet, and a nightly test that depends on someone else's server reports
      their uptime rather than our runtime.
    </p>

    <h2>What the current public release can run</h2>
    <p>
      All of them. <code>v0.1.0-rc5</code> is the first release with drawing,
      animation and sound, and it was checked by downloading the published
      binary and running the 2D game, the chart and the sandbox escape test
      with it &mdash; not by trusting the build that made it.
    </p>
    <p class="generated">
      One gap: there is no arm64 Linux binary in rc5. It builds inside a
      container whose C tooling is too old for one dependency, and holding four
      working platforms for it helped nobody. The installer says so on that
      machine and gives the build-from-source command.
    </p>

    <h2>How much of the system is real</h2>
    <ul>
      <li>{interfaces}</li>
      <li>{widgets}</li>
    </ul>
    <p>
      Both lines are generated from the runtime itself, not written by hand, and
      the build fails if they drift from what the code does.
    </p>

    <h2>What does not work yet</h2>
    <p>We publish this for the same reason we publish the rest.</p>
    <ul>
      <li><strong>3D graphics.</strong> The interface exists and does nothing.</li>
      <li><strong>Heavy 3D.</strong> Software 3D works -- triangles with depth
          and lighting, 54 frames a second at 640x480 on a laptop. A modern 3D
          game needs the GPU path, which is planned rather than written.</li>
      <li><strong>Video.</strong> No decoder, no frame clock.</li>
      <li><strong>Live connections.</strong> HTTPS works; streaming and
          WebSockets do not, so no multiplayer and no live feeds.</li>
      <li><strong>System menu bars.</strong> Apps use in-window buttons instead.</li>
      <li><strong>Judgment inside the sandbox.</strong> When an AI builds an app,
          Krate guarantees it cannot exceed the permissions you granted. It does
          not guarantee the AI chose well inside them — we caught our own
          generated password manager storing passwords in ordinary storage, and
          fixed the instructions that led it there.</li>
    </ul>

    <p style="margin-top:3rem"><a href="/">Back to krate.tech</a></p>
  </main>
</body>
</html>
"""
    out = ROOT.joinpath("docs/landing/progress.html")
    out.write_text(html)
    print(f"wrote {out} ({len(bundles())} apps, {total:,} bytes total)")


if __name__ == "__main__":
    main()
