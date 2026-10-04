#!/usr/bin/env python3
"""Generate a truthful source inventory, not an unexecuted release test result."""
import argparse
import datetime
from html import escape
import pathlib
import re
import subprocess

from public_facts import ROOT, bundle_inventory, load_facts, published_release, source_url
import site_kit as K  # the header, phone menu and footer every kit page shares


def wbr_path(path):
    """A repository path that wraps after each slash with the file name whole."""
    *dirs, name = path.split("/")
    return "".join(escape(d) + "/<wbr>" for d in dirs) + f'<span class="fn">{escape(name)}</span>'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=pathlib.Path, default=ROOT / "docs/landing/progress.html")
    args = parser.parse_args()
    facts = load_facts()
    inventory = bundle_inventory()
    total = sum(row["bundle_bytes"] for row in inventory)
    inventory_rows = "\n".join(
        f'<tr><td><a href="{source_url(r["path"])}"><code>{wbr_path(r["path"])}</code></a></td>'
        f'<td class="num">{r["bundle_bytes"]:,}</td><td class="num">{r["code_bytes"]:,}</td>'
        f'<td>{"Yes" if r["source"] else "No"}</td></tr>' for r in inventory)
    script_path = "scripts/replay-ported-apps.sh"
    script_url = source_url(script_path)
    replay_names = sorted(set(re.findall(r'^check "([a-z0-9-]+)"',
                                         (ROOT / script_path).read_text(), re.M)))
    replay_list = "".join(f'<span class="tag">{escape(name)}</span>' for name in replay_names)
    version = published_release()
    release_line = (f'Latest published release at generation: <a href="{facts["latest_release_url"]}">'
                    f'<code>{escape(version)}</code></a>.' if version else
                    f'See the <a href="{facts["latest_release_url"]}">latest published release</a>'
                    ' for current downloads. Release version was not resolved during generation.')
    revision = subprocess.run(["git", "rev-parse", "--short", "HEAD"], cwd=ROOT,
                              capture_output=True, text=True, check=True).stdout.strip()
    today = datetime.date.today().isoformat()
    release_tile = (f'<a href="{facts["latest_release_url"]}">{escape(version)}</a>' if version
                    else f'<a href="{facts["latest_release_url"]}">Latest</a>')
    arrow = K.ARROW
    html = f"""<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover">
<meta name="description" content="Krate repository inventory: complete app bundle sizes, source inclusion, test configuration, release links and current capability references.">
<link rel="canonical" href="https://krate.tech/progress/">
<meta property="og:title" content="Krate: repository inventory and releases">
<meta property="og:description" content="App artifacts and their complete sizes, generated from this checkout. Release support and test results are linked separately.">
<meta property="og:type" content="website">
<meta property="og:url" content="https://krate.tech/progress/">
<title>Krate repository inventory and release links</title>
<link rel="icon" href="/krate-favicon.png">
<link rel="apple-touch-icon" href="/krate-favicon.png">
<meta name="theme-color" content="#fbfbfd" media="(prefers-color-scheme: light)">
<meta name="theme-color" content="#08080a" media="(prefers-color-scheme: dark)">
{K.HEAD_THEME}
{K.KIT_LINKS}
<style>
.ph .meta {{ max-width: 560px; margin: 16px auto 0; line-height: 1.6; }}
.ph .meta a {{ color: var(--accent); }}
.ph .lede + .lede {{ margin-top: 10px; }}
.stats-sec .stats .stat b {{ font-variant-numeric: tabular-nums; }}
.stats-sec .stat a {{ color: inherit; }}
.stats-sec .stat a:hover {{ color: var(--accent); }}
.psec + .psec {{ margin-top: 88px; }}
.prose > .eye {{ display: flex; }}
.prose .eye + h2 {{ margin-top: 8px; }}
.prose h2 {{ font-size: clamp(26px, 2.6vw, 32px); letter-spacing: -.035em; }}
.prose .tblw, .prose .call, .prose .checks, .prose .links {{ margin-top: 24px; }}
.method {{ font-size: 14px; line-height: 1.65; color: var(--ink-3); }}
.prose .tbl a {{ text-decoration: none; }}
.prose .tbl code {{ padding: 0; background: none; color: var(--accent); font-size: 12.5px; white-space: nowrap; }}
.prose .tbl th {{ white-space: nowrap; }}
.prose .tbl td:first-child {{ white-space: nowrap; }}
.checks {{ display: flex; flex-wrap: wrap; gap: 8px; }}
.checks .tag {{ height: 28px; padding: 0 11px; font: 500 12.5px ui-monospace, "SF Mono", Menlo, monospace; }}
.prose .links {{ display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 12px; list-style: none; padding: 0; }}
.prose .links li {{ margin: 0; }}
.prose .links a {{ display: flex; align-items: center; justify-content: space-between; gap: 12px; height: 56px; padding: 0 18px; border-radius: 16px; background: var(--s1); box-shadow: var(--sh-1); color: var(--ink); font-size: 15px; font-weight: 500; text-decoration: none; transition: box-shadow .3s, transform .45s var(--ease); }}
.prose .links a:hover {{ box-shadow: var(--sh-2); transform: translateY(-2px); }}
.prose .links a svg {{ width: 12px; height: 12px; flex: none; color: var(--ink-3); }}
.back {{ margin-top: 48px; padding-top: 24px; border-top: 1px solid var(--line); }}
.back .btn svg {{ transform: scaleX(-1); }}
.tbl .fn {{ white-space: nowrap; }}
@media (max-width: 680px) {{ .prose .links {{ grid-template-columns: 1fr; }} .stats-sec .stats {{ gap: 10px; }} .stats-sec .stat {{ padding: 18px 16px 16px; }} .stats-sec .stat b {{ font-size: 27px; }} }}
@media (max-width: 680px) {{ .prose .tbl {{ font-size: 13px; }} .prose .tbl th {{ white-space: normal; padding: 10px 8px; }} .prose .tbl td {{ padding: 12px 8px; }} .prose .tbl th:first-child, .prose .tbl td:first-child {{ padding-left: 14px; }} .prose .tbl td:first-child {{ white-space: normal; }} .prose .tbl code {{ white-space: normal; overflow-wrap: break-word; font-size: 11.5px; }} }}
</style>
</head>
<body>
{K.HEADER}
{K.MNAV}

<main>
<section class="ph wrap n">
  <span class="eye">Repository inventory</span>
  <h1>What is in this checkout.</h1>
  <p class="lede">{escape(facts["description"])}</p>
  <p class="lede">{escape(facts["prerequisite"])}</p>
  <p class="meta">{release_line} Generated {today} from commit <code class="k">{revision}</code>. This is a source snapshot, not a fresh cross-platform test run.</p>
</section>

<section class="sec t wrap stats-sec">
  <div class="stats rv">
    <div class="stat"><b>{release_tile}</b><span>Latest published release at generation</span></div>
    <div class="stat"><b>{len(inventory)}</b><span>Artifacts in this checkout</span></div>
    <div class="stat"><b>{total:,}</b><span>Full-bundle bytes in total</span></div>
    <div class="stat"><b>{len(replay_names)}</b><span>Checks named by the replay script</span></div>
  </div>
</section>

<div class="sec t wrap n">
  <section class="prose psec">
    <span class="eye">Files</span>
    <h2>Application artifacts</h2>
    <div class="tblw">
      <table class="tbl">
        <thead><tr><th>Artifact</th><th class="num">Full bundle bytes</th><th class="num">Code payload bytes</th><th>Carries source</th></tr></thead>
        <tbody>
{inventory_rows}
        </tbody>
      </table>
    </div>
    <p>{len(inventory)} artifacts, {total:,} full-bundle bytes in total. These are files, not a count of unique applications or confirmed users. Apps with the same name can have different builds on different shelves.</p>
    <p class="method">{escape(facts["bundle_accounting"])} Code payload means the uncompressed <code>code.wasm</code> entry; older bundles without source are identified instead of presented as the size of all current apps. The shared runtime is not included.</p>
  </section>

  <section class="prose psec">
    <span class="eye">Tests</span>
    <h2>Test configuration is not a test result</h2>
    <p>The repository's ported-app replay script names {len(replay_names)} checks:</p>
    <div class="checks">{replay_list}</div>
    <p>This list says what the script is configured to exercise, not whether the latest run passed.</p>
    <p>Inspect <a href="{script_url}">the replay script</a>, <a href="{facts["repository"]}/actions/workflows/ci.yml">CI run results</a> and <a href="{facts["latest_release_url"]}">release notes</a> for the revision and host you intend to use. A file existing in this checkout does not prove it works with every published runtime version.</p>
  </section>

  <section class="prose psec">
    <span class="eye">Where to start</span>
    <h2>Choose a supported application</h2>
    <p>Start with the <a href="/docs/porting.html">porting guide</a> and <a href="/docs/limits.html">current limits</a>. Runtime APIs and bundle formats evolve; use a compatible runtime, SDK and artifact.</p>
    <ul class="links">
      <li><a href="/docs/reference/interface-parity.html">Interface coverage {arrow}</a></li>
      <li><a href="/docs/reference/widget-parity.html">Widget coverage {arrow}</a></li>
      <li><a href="/docs/quickstart.html">Developer quickstart {arrow}</a></li>
      <li><a href="/studio/">AI-assisted authoring in Studio {arrow}</a></li>
    </ul>
    <div class="call"><svg class="ci" viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M10 2.5l6 2.7v4.3c0 3.8-2.6 6.6-6 7.8-3.4-1.2-6-4-6-7.8V5.2z"/></svg><span>{escape(facts["security"])}</span></div>
  </section>

  <section class="prose psec">
    <span class="eye">Speed and size</span>
    <h2>Performance needs its own evidence</h2>
    <p>The <a href="/reports/">measurement report</a> describes one architecture-matched notes workload and separates full bundle, code payload and shared-runtime costs. This inventory makes no speed, memory or size-ratio claim against unrelated applications.</p>
    <div class="back"><a class="btn link" href="/">{arrow} Back to krate.tech</a></div>
  </section>
</div>
</main>

{K.FOOTER}
{K.KIT_SCRIPT}
</body>
</html>
"""
    args.output.write_text(html)
    print(f"wrote {args.output} ({len(inventory)} artifacts, {total:,} full-bundle bytes)")


if __name__ == "__main__":
    main()
