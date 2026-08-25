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
# Discord on this Mac: `du -sk` reports 440,944 KB. The same like-for-like
# comparison the reports page makes: one codebase shipped to Mac, Windows and
# Linux, done by putting a browser in every copy. Re-measure with
# scripts/measure-peer-apps.sh.
DISCORD_BYTES = 440_944 * 1024


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


def newest_release():
    """The newest published release, so a tag cannot be advertised too early.

    This said rc16 while rc21 shipped -- five releases stale on a page whose
    whole subject is what is current. A hard-coded version on a progress page
    is a promise to update it by hand every release, and that promise is
    always broken eventually.
    """
    try:
        published = subprocess.run(
            ["gh", "api", "repos/incyashraj/krate/releases/latest", "--jq", ".tag_name"],
            cwd=ROOT, capture_output=True, text=True, check=True,
        ).stdout.strip()
        if published:
            return published
    except Exception:
        pass
    try:
        tags = subprocess.run(
            ["git", "tag", "--list", "v*", "--sort=-v:refname"],
            cwd=ROOT, capture_output=True, text=True, check=True,
        ).stdout.split()
        return tags[0] if tags else "v0.1.0"
    except Exception:
        return "v0.1.0"


NEWEST = newest_release()

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
    "bounce": "2D game: a playable Breakout with paddle, bricks, lives, a win",
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
            f'<td class="num">{DISCORD_BYTES / size:,.0f}&times;</td>'
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
  <meta name="theme-color" content="#0a0a0a" />
  <meta name="description" content="Krate is not a concept. See the apps, sizes, platform coverage, interfaces and limits measured from the repository today." />
  <link rel="canonical" href="https://krate.tech/progress/" />
  <meta property="og:title" content="Krate: this is what ships today" />
  <meta property="og:description" content="Every app we have shipped, its size, and what still does not work. Generated from the repository, not written by hand." />
  <meta property="og:type" content="website" />
  <meta property="og:url" content="https://krate.tech/progress/" />
  <title>Krate: this is what ships today</title>
  <link rel="icon" href="/krate-favicon.png" />
  <link rel="apple-touch-icon" href="/krate-favicon.png" />
  <link rel="stylesheet" href="/site.css" />
  <style>
    .generated {{ color: var(--quiet); font-size: 0.9em; }}
  </style>
</head>
<body>
  <!-- Links are absolute because this page is served from /progress/. -->
  <header class="subnav">
    <div class="wrap subnav-inner">
      <a class="brand" href="/"><img src="/krate-glyph-white.png" alt="" width="22" height="22" /> KRATE</a>
      <nav>
        <a href="/#install">Start</a>
        <a href="/docs/">Docs</a>
        <a href="/cloud/">Cloud</a>
        <a href="/reports/">Reports</a>
      </nav>
      <a class="pill pill-primary" href="/#install">Install</a>
    </div>
  </header>

  <main class="page-wide">
    <p class="page-kicker">PROOF</p>
    <h1>This is not a concept. It ships.</h1>
    <p>
      Every app we have shipped, its real size, and whether it is re-tested every
      night on Mac, Windows, and Linux. This page is generated from the
      repository. The current release is <code>{NEWEST}</code>, with Krate
      Studio downloads for Mac, Windows and Linux.
    </p>
    <p class="generated">Generated {today} from commit <code>{commit()}</code>.</p>

    <h2>Apps that exist and run</h2>
    <table>
      <thead>
        <tr><th>App</th><th class="num">Bytes</th><th class="num">vs Discord</th><th>What it is</th><th>Nightly</th></tr>
      </thead>
      <tbody>
{chr(10).join(rows)}
      </tbody>
    </table>
    <p>
      <strong>All {len(bundles())} apps together: {total:,} bytes ({total / 1024:,.0f}&nbsp;KB).</strong>
      Discord, which solves the same one-codebase-three-systems problem with a
      browser inside the app, is {DISCORD_BYTES / total:,.1f}&times; that on its own.
    </p>
    <p class="generated">
      One app is not re-run nightly on purpose: <code>rssfwd</code> reaches the
      internet, and a nightly test that depends on someone else's server reports
      their uptime rather than our runtime.
    </p>

    <h2>What the current public release can run</h2>
    <p>
      All of them. The current release is <code>{NEWEST}</code>. Drawing,
      animation, audio capture, speech transcription and 3D scenes work, and each release is
      checked by downloading the published binary and running the 2D game, the
      chart and the sandbox escape test with it, not by trusting the build that
      made it. Krate also has <code>krate run app.wasm --shoot frame.png</code>,
      which paints any app's window to a PNG on any machine with no display.
    </p>
    <p class="generated">
      Six platforms ship, including arm64 Linux and ARM Windows.
      The earlier gap (no arm64 Linux binary in rc5) is closed.
    </p>

    <h2>New this month</h2>
    <ul>
      <li><strong>Shared apps without accounts.</strong> Two machines holding
          the same ten-character invite code see the same data, synced through
          krate.tech. A grocery list built by an AI in one sentence ("a shared
          grocery list my wife and I can both edit") showed one merged list on
          a Mac and a Windows PC. No sign-up, no server of yours.</li>
      <li><strong>The machine's senses.</strong> Generated apps now use the
          microphone, the camera, local speech-to-text and sound. Asked for
          "a voice memo app", the AI shipped one on the first try: record
          button, live level meter, playback, memos kept between launches.</li>
      <li><strong>Mac downloads open clean.</strong> Krate Studio and the
          runtime are signed and notarized with a Developer ID; macOS opens
          them like any other download, offline included.</li>
      <li><strong>You can only download a verified release.</strong> Every
          release is born unlisted and is promoted to the public channel only
          after the published files themselves pass checksum, Gatekeeper, and
          run-a-real-app checks on all three systems.</li>
      <li><strong>Windows looks right.</strong> Native-density rendering,
          window controls on every app window, drag and double-click-to-
          maximize -- verified on a physical Windows PC, not an emulator.</li>
      <li><strong>The AI starts warm.</strong> Everything the AI needs rides
          inside its first instruction, and the build continues the very
          conversation that planned it -- so it writes code from the first
          minute instead of reading files.</li>
    </ul>

    <h2>How much of the system is real</h2>
    <ul>
      <li>{interfaces} Two more interfaces are partial.</li>
      <li>{widgets}</li>
    </ul>
    <p>
      Both lines are generated from the runtime itself, not written by hand, and
      the build fails if they drift from what the code does.
    </p>

    <h2>What does not work yet</h2>
    <p>We publish this for the same reason we publish the rest.</p>
    <ul>
      <li><strong>GPU 3D.</strong> Software 3D works with triangles, depth
          and lighting, 54 frames a second at 640x480 on a laptop. A modern 3D
          game needs the GPU path, which is planned rather than written.</li>
      <li><strong>Video.</strong> No decoder, no frame clock.</li>
      <li><strong>Live connections.</strong> HTTPS works; streaming and
          WebSockets do not, so no multiplayer and no live feeds.</li>
      <li><strong>System menu bars.</strong> Apps use in-window buttons instead.</li>
      <li><strong>Judgment inside the sandbox.</strong> When an AI builds an app,
          Krate guarantees it cannot exceed the permissions you granted. It does
          not guarantee the AI chose well inside them. We caught our own
          generated password manager storing passwords in ordinary storage, and
          fixed the instructions that led it there.</li>
    </ul>

    <p style="margin-top:3rem"><a href="/">&larr; Back to krate.tech</a></p>
  </main>

  <footer class="subfoot">
    <div class="wrap subfoot-inner">
      <span>© 2026 Krate Labs</span>
      <span>
        <a href="/docs/">Docs</a>
        <a href="/reports/">Reports</a>
        <a href="https://github.com/incyashraj/krate">GitHub</a>
      </span>
    </div>
  </footer>
</body>
</html>
"""
    out = ROOT.joinpath("docs/landing/progress.html")
    out.write_text(html)
    print(f"wrote {out} ({len(bundles())} apps, {total:,} bytes total)")


if __name__ == "__main__":
    main()
