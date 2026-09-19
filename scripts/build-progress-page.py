#!/usr/bin/env python3
"""Generate a truthful source inventory, not an unexecuted release test result."""
import argparse
import datetime
from html import escape
import pathlib
import re
import subprocess

from public_facts import ROOT, bundle_inventory, load_facts, published_release, source_url


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=pathlib.Path, default=ROOT / "docs/landing/progress.html")
    args = parser.parse_args()
    facts = load_facts()
    inventory = bundle_inventory()
    total = sum(row["bundle_bytes"] for row in inventory)
    inventory_rows = "\n".join(
        f'<tr><td><a href="{source_url(r["path"])}"><code>{escape(r["path"])}</code></a></td>'
        f'<td class="num">{r["bundle_bytes"]:,}</td><td class="num">{r["code_bytes"]:,}</td>'
        f'<td>{"Yes" if r["source"] else "No"}</td></tr>' for r in inventory)
    script_path = "scripts/replay-ported-apps.sh"
    script_url = source_url(script_path)
    replay_names = sorted(set(re.findall(r'^check "([a-z0-9-]+)"',
                                         (ROOT / script_path).read_text(), re.M)))
    replay_list = ", ".join(f"<code>{escape(name)}</code>" for name in replay_names)
    version = published_release()
    release_line = (f'Latest published release at generation: <a href="{facts["latest_release_url"]}">'
                    f'<code>{escape(version)}</code></a>.' if version else
                    f'See the <a href="{facts["latest_release_url"]}">latest published release</a>'
                    ' for current downloads. Release version was not resolved during generation.')
    revision = subprocess.run(["git", "rev-parse", "--short", "HEAD"], cwd=ROOT,
                              capture_output=True, text=True, check=True).stdout.strip()
    today = datetime.date.today().isoformat()
    html = f"""<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <meta name="theme-color" content="#0a0a0a" />
  <meta name="description" content="Krate repository inventory: complete app bundle sizes, source inclusion, test configuration, release links and current capability references." />
  <link rel="canonical" href="https://krate.tech/progress/" />
  <meta property="og:title" content="Krate: repository inventory and releases" />
  <meta property="og:description" content="App artifacts and their complete sizes, generated from this checkout. Release support and test results are linked separately." />
  <meta property="og:type" content="website" />
  <meta property="og:url" content="https://krate.tech/progress/" />
  <title>Krate repository inventory and release links</title>
  <link rel="icon" href="/krate-favicon.png" />
  <link rel="apple-touch-icon" href="/krate-favicon.png" />
  <style>
  @font-face {{ font-family: "Geist"; src: url("/fonts/geist-400.woff2") format("woff2"); font-weight: 400; font-style: normal; font-display: swap; }}
  @font-face {{ font-family: "Geist"; src: url("/fonts/geist-500.woff2") format("woff2"); font-weight: 500; font-style: normal; font-display: swap; }}
  :root {{
    color-scheme: dark;
    --bg: #0a0a0a; --panel: #0f1012; --panel-raised: #16171b;
    --line: #1f2228; --line-strong: #2e323a; --line-soft: #17191e;
    --text: #ffffff; --muted: rgba(255,255,255,0.55); --quiet: rgba(255,255,255,0.35);
    --accent: #6291ff; --r-pill: 999px;
    --sans: "Geist", Inter, ui-sans-serif, system-ui, -apple-system, "Segoe UI", sans-serif;
    --mono: "SFMono-Regular", "Cascadia Code", Menlo, Consolas, monospace;
  }}
  * {{ box-sizing: border-box; margin: 0; padding: 0; }}
  body {{ background: var(--bg); color: var(--text); font-family: var(--sans); font-size: 16px; line-height: 1.6; -webkit-font-smoothing: antialiased; }}
  a {{ color: inherit; text-decoration: none; }}
  img {{ max-width: 100%; display: block; }}
  h1, h2, h3 {{ font-weight: 500; letter-spacing: -0.02em; }}
  .wrap {{ max-width: 1120px; margin: 0 auto; padding: 0 24px; }}
  .site-head {{ position: sticky; top: 0; z-index: 40; background: color-mix(in srgb, var(--bg) 86%, transparent); backdrop-filter: blur(14px); -webkit-backdrop-filter: blur(14px); border-bottom: 1px solid var(--line-soft); }}
  .head-inner {{ display: flex; align-items: center; gap: 24px; height: 64px; max-width: 1120px; margin: 0 auto; padding: 0 24px; }}
  .brand {{ display: flex; align-items: center; gap: 10px; font-weight: 500; letter-spacing: 0.14em; font-size: 14px; }}
  .head-nav {{ display: flex; gap: 4px; margin-left: auto; }}
  .head-nav a {{ font-size: 14px; color: var(--muted); padding: 8px 12px; border-radius: var(--r-pill); }}
  .head-nav a:hover {{ color: var(--text); background: rgba(255,255,255,0.05); }}
  @media (max-width: 640px) {{ .head-nav {{ display: none; }} }}
  .pill {{ display: inline-flex; align-items: center; padding: 9px 18px; border-radius: var(--r-pill); border: 1px solid var(--line); font-size: 14.5px; font-weight: 500; }}
  .pill-primary {{ background: var(--text); color: #0a0a0a; border-color: var(--text); }}
  .pill-primary:hover {{ background: rgba(255,255,255,0.86); }}
  .page-wide {{ max-width: 900px; margin: 0 auto; padding: clamp(48px, 8vh, 84px) 24px clamp(64px, 10vh, 100px); }}
  .page-kicker {{ font-size: 12px; letter-spacing: 0.14em; color: var(--quiet); }}
  .page-wide h1 {{ margin-top: 10px; font-size: clamp(30px, 4.6vw, 46px); letter-spacing: -0.028em; line-height: 1.08; }}
  .page-wide h2 {{ margin-top: 44px; font-size: 21px; }}
  .page-wide > main p, .page-wide p {{ color: var(--muted); }}
  .page-wide h1 + p {{ margin-top: 14px; max-width: 44em; font-size: 16.5px; }}
  .page-wide table {{ border-collapse: collapse; width: 100%; margin: 18px 0 10px; font-size: 14.5px; }}
  .page-wide th, .page-wide td {{ padding: 10px 12px; text-align: left; border-bottom: 1px solid var(--line-soft); }}
  .page-wide th {{ font-size: 12px; text-transform: uppercase; letter-spacing: 0.06em; color: var(--quiet); font-weight: 500; }}
  .page-wide td {{ color: var(--muted); }}
  .page-wide td:first-child {{ color: var(--text); }}
  .num {{ text-align: right; font-variant-numeric: tabular-nums; }}
  /* A wide table scrolls inside this box instead of widening the page.
     `width: max-content` stops the table shrinking its columns into a
     tall unreadable stack, so it keeps one row per app and slides. */
  .table-scroll {{ overflow-x: auto; -webkit-overflow-scrolling: touch;
                   overscroll-behavior-x: contain; }}
  .table-scroll > table {{ min-width: max-content; }}
  /* Nothing else may widen the page either: a long bundle name or a URL
     in a cell is the other way this starts scrolling sideways. */
  .page-wide {{ overflow-x: clip; }}
  .page-wide :is(p, li, h1, h2, h3) {{ overflow-wrap: anywhere; }}
  code {{ font-family: var(--mono); font-size: 0.86em; }}
  ul, ol {{ padding-left: 1.3em; color: var(--muted); }}
  li {{ margin: 4px 0; }}
  </style>
  <style>
    .generated {{ color: var(--quiet); font-size: 0.9em; }}
  </style>
</head>
<body>
  <!-- Links are absolute because this page is served from /progress/. -->
  <header class="site-head">
  <div class="head-inner">
    <a class="brand" href="/">
      <img src="/krate-glyph-white.png" alt="" width="22" height="22" />
      KRATE
    </a>
    <nav class="head-nav" aria-label="Primary">
      <a href="/#how">How it works</a>
      <a href="/cloud/">Gallery</a>
      <a href="/docs/">Docs</a>
      <a href="https://github.com/incyashraj/krate">GitHub</a>
    </nav>
    <a class="pill pill-primary" href="/studio/">Download Studio</a>
  </div>
</header>

  <main class="page-wide">
    <p class="page-kicker">REPOSITORY INVENTORY</p>
    <h1>What is in this checkout.</h1>
    <p>{escape(facts["description"])}</p>
    <p>{escape(facts["prerequisite"])}</p>
    <p>{release_line}</p>
    <p class="generated">Generated {today} from commit <code>{revision}</code>.
      This is a source snapshot, not a fresh cross-platform test run.</p>

    <h2>Application artifacts</h2>
    <div class="table-scroll">
      <table>
        <thead><tr><th>Artifact</th><th class="num">Full bundle bytes</th>
          <th class="num">Code payload bytes</th><th>Carries source</th></tr></thead>
        <tbody>{inventory_rows}</tbody>
      </table>
    </div>
    <p>{len(inventory)} artifacts, {total:,} full-bundle bytes in total.
      These are files, not a count of unique applications or confirmed users.
      Apps with the same name can have different builds on different shelves.</p>
    <p class="generated">{escape(facts["bundle_accounting"])}
      Code payload means the uncompressed <code>code.wasm</code> entry;
      older bundles without source are identified instead of presented as the
      size of all current apps. The shared runtime is not included.</p>

    <h2>Test configuration is not a test result</h2>
    <p>The repository's ported-app replay script names {len(replay_names)}
      checks: {replay_list}. This list says what the script is configured to
      exercise, not whether the latest run passed.</p>
    <p>Inspect <a href="{script_url}">the replay script</a>,
      <a href="{facts["repository"]}/actions/workflows/ci.yml">CI run results</a>
      and <a href="{facts["latest_release_url"]}">release notes</a> for the
      revision and host you intend to use. A file existing in this checkout
      does not prove it works with every published runtime version.</p>

    <h2>Choose a supported application</h2>
    <p>Start with the <a href="/docs/porting.html">porting guide</a> and
      <a href="/docs/limits.html">current limits</a>. Runtime APIs and bundle
      formats evolve; use a compatible runtime, SDK and artifact.</p>
    <ul>
      <li><a href="/docs/reference/interface-parity.html">Interface coverage</a></li>
      <li><a href="/docs/reference/widget-parity.html">Widget coverage</a></li>
      <li><a href="/docs/quickstart.html">Developer quickstart</a></li>
      <li><a href="/studio/">AI-assisted authoring in Studio</a></li>
    </ul>
    <p>{escape(facts["security"])}</p>

    <h2>Performance needs its own evidence</h2>
    <p>The <a href="/reports/">measurement report</a> describes one
      architecture-matched notes workload and separates full bundle, code
      payload and shared-runtime costs. This inventory makes no speed, memory
      or size-ratio claim against unrelated applications.</p>
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
    args.output.write_text(html)
    print(f"wrote {args.output} ({len(inventory)} artifacts, {total:,} full-bundle bytes)")


if __name__ == "__main__":
    main()
