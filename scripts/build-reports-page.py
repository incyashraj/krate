#!/usr/bin/env python3
"""Generate reports from scoped public claims and complete bundle accounting.

No live apps are executed and no local application sizes are compared.
--no-run is retained for compatibility with the Pages workflow.
"""
import argparse
import datetime
import pathlib
import re
import subprocess

from public_facts import ROOT, benchmark_claims, bundle_inventory, load_facts, source_url
import site_kit as K  # the header, phone menu and footer every kit page shares

def add_contents(page):
    """Give every report section's <h2> an id and list them in the side
    contents (the kit's .toc, which marks where the reader is).

    This page is ten screens and more on a phone; without a contents list the
    only way to reach the last section was to scroll past every other one.
    Derived from the headings themselves, so a renamed or reordered section
    cannot leave a contents link pointing at something that is gone.
    """
    heads = []

    def slug(text):
        s = re.sub(r"<[^>]+>", "", text)
        s = s.replace("&ndash;", "-").replace("&nbsp;", " ").replace("&amp;", "&")
        s = re.sub(r"[^a-z0-9]+", "-", s.lower()).strip("-")
        return s[:48] or "section"

    def tag(m):
        inner = m.group(1)
        base = slug(inner)
        ident = base
        n = 2
        while ident in [h[0] for h in heads]:
            ident, n = f"{base}-{n}", n + 1
        label = re.sub(r"<[^>]+>", "", inner)
        label = label.replace("&ndash;", "-").replace("&nbsp;", " ").replace("&amp;", "&")
        heads.append((ident, label.strip().rstrip(".")))
        return f'<h2 id="{ident}">{inner}</h2>'

    page = re.sub(r"<h2>(.*?)</h2>", tag, page, flags=re.S)
    items = "\n".join(f'      <a href="#{i}">{html_escape(t)}</a>' for i, t in heads)
    return page.replace("<!--TOC-->", items, 1)


def stat_tiles(claims):
    """The four headline numbers, each read from the claim record that the
    table below quotes, never typed into the page. A record whose wording no
    longer yields a number fails the build rather than showing a stale one."""
    by = {c["id"]: c for c in claims}
    warm, mem, size = by["warm-start-50k"], by["memory-50k"], by["app-file-size"]

    def first(pattern, text, what):
        m = re.search(pattern, text)
        if not m:
            raise SystemExit(f"build-reports-page: cannot read {what} from the claim record: {text!r}")
        return m.group(1)

    runtime = first(r"installed once at ([0-9.,]+ MiB)", size["caveat"], "the shared runtime size")
    ratio = first(r"first-app disk ratio ([0-9.]+x)", size["caveat"], "the first-app disk ratio")
    not_ratio = first(r"not ([0-9.,]+x)", size["caveat"], "the payload ratio it is not")
    warm_us = first(r"^([0-9.,]+ ms)", warm["us"], "Krate's warm open")
    mem_us = first(r"^([0-9.,]+ MiB)", mem["us"], "Krate's memory")
    tiles = [
        (warm_us, "Warm open, 50,000 lines, median", f"MarkText: {warm['them']}"),
        (mem_us, "Memory, 50,000 lines" + (", one process" if "one process" in mem["us"] else ""), f"MarkText: {mem['them']}"),
        (runtime, "Shared Krate runtime, installed once", f"Per-app payload: {size['us']}"),
        (ratio, "First-app disk ratio", f"Not {not_ratio}"),
    ]
    return "\n".join(
        f'          <div class="stat"><b>{html_escape(b)}</b><span>{html_escape(l)}</span><span class="vs">{html_escape(v)}</span></div>'
        for b, l, v in tiles)


def wbr_path(path):
    """evidence/store/krate-notes.krate with break points after each slash and
    the file name kept whole, so a phone wraps the column instead of the page."""
    *dirs, name = path.split("/")
    return "".join(html_escape(d) + "/<wbr>" for d in dirs) + f'<span class="fn">{html_escape(name)}</span>'


def html_escape(s):
    return s.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")



def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--no-run", action="store_true", help="Compatibility flag; generation never benchmarks")
    parser.add_argument("--output", type=pathlib.Path, default=ROOT / "docs/landing/reports.html")
    args = parser.parse_args()
    facts = load_facts()
    claims = benchmark_claims()
    inventory = bundle_inventory()
    labels = {"app-file-size": "Historical app bundle versus installed application",
              "memory-50k": "Memory, 50,000 lines", "warm-start-50k": "Warm open, 50,000 lines"}
    benchmark_rows = "\n".join(
        f'<tr><td>{labels[c["id"]]}</td><td>{html_escape(c["us"])}</td>'
        f'<td>{html_escape(c["them"])}</td></tr>' for c in claims)
    inventory_rows = "\n".join(
        f'<tr><td><a href="{source_url(r["path"])}"><code>{wbr_path(r["path"])}</code></a></td>'
        f'<td class="num">{r["bundle_bytes"]:,}</td><td class="num">{r["code_bytes"]:,}</td>'
        f'<td>{"Yes" if r["source"] else "No"}</td></tr>' for r in inventory)
    analysis_url = source_url(claims[0]["source"])
    kit_url = source_url("evidence/benchmarks/marktext-vs-krate/README.md")
    audit_url = source_url(claims[0]["evidence_run"] + "/audit.txt")
    seal_url = source_url(claims[0]["seal"])
    revision = subprocess.run(["git", "rev-parse", "--short", "HEAD"], cwd=ROOT,
                              capture_output=True, text=True, check=True).stdout.strip()
    today = datetime.date.today().isoformat()
    tiles = stat_tiles(claims)
    html = f"""<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover">
<meta name="description" content="Krate benchmark methodology: a scoped MarkText notes comparison, complete bundle sizes, code payload and shared runtime costs.">
<link rel="canonical" href="https://krate.tech/reports/">
<meta property="og:title" content="Krate: the measurements">
<meta property="og:description" content="A measured notes workload, with methods and limitations. Full application bundles and shared runtime costs are shown separately.">
<meta property="og:type" content="website">
<meta property="og:url" content="https://krate.tech/reports/">
<meta property="og:image" content="https://krate.tech/og-v2.png">
<meta name="twitter:card" content="summary_large_image">
<title>Krate: the measurements</title>
<link rel="icon" href="/krate-favicon.png">
<link rel="apple-touch-icon" href="/krate-favicon.png">
<meta name="theme-color" content="#fbfbfd" media="(prefers-color-scheme: light)">
<meta name="theme-color" content="#08080a" media="(prefers-color-scheme: dark)">
{K.HEAD_THEME}
{K.KIT_LINKS}
<style>
.ph .meta {{ max-width: 560px; margin: 16px auto 0; line-height: 1.6; }}
.ph .meta code.k, .method code {{ font-size: .92em; }}
.rsec + .rsec {{ margin-top: 96px; }}
.prose > .eye {{ display: flex; }}
.prose .eye + h2 {{ margin-top: 8px; }}
.prose h2 {{ font-size: clamp(26px, 2.6vw, 32px); letter-spacing: -.035em; scroll-margin-top: calc(var(--hd) + 20px); }}
.prose .stats, .prose .tblw, .prose .call, .prose .ev, .prose .steps {{ margin-top: 24px; }}
.doc .stats {{ grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 14px; }}
.prose .stat b {{ font-weight: 500; color: var(--ink); font-variant-numeric: tabular-nums; }}
.stat .vs {{ margin-top: 2px; color: var(--ink-3); }}
.method {{ font-size: 14px; line-height: 1.65; color: var(--ink-3); }}
.prose .tbl a {{ text-decoration: none; }}
.prose .tbl code {{ padding: 0; background: none; color: var(--accent); font-size: 12.5px; white-space: nowrap; }}
.prose .tbl td:first-child {{ white-space: nowrap; }}
.tbl.cmp td:first-child {{ white-space: normal; min-width: 180px; }}
.tbl.cmp {{ min-width: 540px; }}
.tbl.cmp th:first-child {{ width: 40%; }}
.prose .tbl th {{ white-space: nowrap; }}
.tbl.cmp td:nth-child(2) {{ color: var(--ink); }}
.tblw.tall {{ max-height: 560px; overflow-y: auto; }}
.tblw.tall thead th {{ position: sticky; top: 0; background: var(--s1); z-index: 1; }}
.prose .call b {{ font-weight: 600; }}
.ev {{ display: flex; flex-wrap: wrap; gap: 8px; }}
.prose a.btn {{ text-decoration: none; }}
.prose a.btn.ghost {{ color: var(--ink); }}
.prose ol.steps {{ list-style: none; padding: 0; counter-reset: s; display: grid; gap: 10px; }}
.steps li {{ counter-increment: s; position: relative; margin: 0; padding: 16px 18px 16px 58px; border-radius: 16px; background: var(--soft); color: var(--ink-2); line-height: 1.6; }}
.steps li::before {{ content: counter(s); position: absolute; left: 16px; top: 14px; width: 28px; height: 28px; border-radius: 50%; display: grid; place-items: center; font-size: 13px; font-weight: 500; color: var(--accent); background: var(--accent-soft); }}
.prose .steps li + li {{ margin-top: 0; }}
.pl {{ margin-top: 56px; padding-top: 28px; border-top: 1px solid var(--line); display: flex; flex-wrap: wrap; align-items: center; gap: 12px 14px; }}
.tbl .fn {{ white-space: nowrap; }}
@media (max-width: 680px) {{ .doc .stats {{ gap: 10px; }} .doc .stat {{ padding: 18px 16px 16px; }} .doc .stat b {{ font-size: 25px; white-space: nowrap; }} .doc .stat span {{ font-size: 13px; }} .steps li {{ padding-left: 52px; }} .steps li::before {{ left: 12px; }} .tblw.tall {{ max-height: 480px; }} }}
@media (max-width: 680px) {{ .prose .tbl:not(.cmp) {{ font-size: 13px; }} .prose .tbl:not(.cmp) th {{ white-space: normal; padding: 10px 8px; }} .prose .tbl:not(.cmp) td {{ padding: 12px 8px; }} .prose .tbl:not(.cmp) th:first-child, .prose .tbl:not(.cmp) td:first-child {{ padding-left: 14px; }} .prose .tbl:not(.cmp) td:first-child {{ white-space: normal; }} .prose .tbl:not(.cmp) code {{ white-space: normal; overflow-wrap: break-word; font-size: 11.5px; }} }}
</style>
</head>
<body>
{K.HEADER}
{K.MNAV}

<main>
<section class="ph wrap n">
  <span class="eye">Reports</span>
  <h1>Measurements you can inspect.</h1>
  <p class="lede">A notes workload compared on the same machine, plus an inventory of application files in this checkout. These are different kinds of evidence: neither establishes a performance result for every Krate app or for the latest release.</p>
  <p class="meta">Page generated {today} from commit <code class="k">{revision}</code>. Benchmark measured {html_escape(claims[0]["measured_on"])}. Building this page does not run a new benchmark.</p>
</section>

<div class="sec t wrap">
  <div class="doc">
    <nav class="toc" aria-label="What is in this report">
      <h6>What is in here</h6>
<!--TOC-->
    </nav>

    <div>
      <section class="prose rsec">
        <span class="eye">One matched workload</span>
        <h2>Krate notes and MarkText.</h2>
        <p>{html_escape(claims[0]["scope"])}. This does not establish full editor feature parity. Warm-open and memory rows refer to the same 50,000-line document, not different document sizes.</p>
        <div class="stats">
{tiles}
        </div>
        <div class="tblw">
          <table class="tbl cmp">
            <thead><tr><th>Measurement</th><th>Krate</th><th>MarkText</th></tr></thead>
            <tbody>{benchmark_rows}</tbody>
          </table>
        </div>
        <p class="method">Warm open is time to a visible window, median of ten runs per app. Memory is settled footprint across the whole process tree. The historical app-bundle row excludes Krate's shared runtime and does not measure a current source-bearing bundle.</p>
        <div class="call warn"><svg class="ci" viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M10 3l7.5 13h-15z"/><path d="M10 8.5v3.5M10 14.4v.1"/></svg><span>{html_escape(claims[0]["caveat"])}. That first-app ratio is the historical runtime-plus-payload accounting, not a current source-bearing bundle installation measurement.</span></div>
        <p>Lower numbers in these rows are useful evidence for this workload, not a universal efficiency claim. Application features, assets, rendering, host calls and data handling can change the result. Energy was not measured.</p>
        <div class="ev">
          <a class="btn ghost sm" href="{analysis_url}">Read the run analysis <span class="ext">↗</span></a>
          <a class="btn ghost sm" href="{kit_url}">Protocol and reproduction scripts <span class="ext">↗</span></a>
          <a class="btn ghost sm" href="{audit_url}">Audit scope <span class="ext">↗</span></a>
          <a class="btn ghost sm" href="{seal_url}">Run seal <span class="ext">↗</span></a>
        </div>
        <p class="method">The public checkout includes the protocol, analysis, machine summary and seal. It does not distribute every retained raw sample. A seal records file identity, not an independent review. Reproduction requires the documented apps, hardware and measurement setup; generating this website is not reproduction of the experiment.</p>
      </section>

      <section class="prose rsec">
        <span class="eye">What the recipient downloads</span>
        <h2>Bundle, code payload and runtime are different costs.</h2>
        <p>{html_escape(facts["bundle_accounting"])}</p>
        <p>{html_escape(facts["prerequisite"])}</p>
        <div class="tblw tall">
          <table class="tbl">
            <thead><tr><th>Repository artifact</th><th class="num">Full bundle bytes</th><th class="num">Code payload bytes</th><th>Carries source</th></tr></thead>
            <tbody>
{inventory_rows}
            </tbody>
          </table>
        </div>
        <p class="method">{len(inventory)} artifacts in this checkout. Full bundle bytes are the actual archive length; code payload is the uncompressed <code>code.wasm</code> entry. Compression means payload bytes are not an additive part of the archive length. Equal app names on different shelves are retained as different artifacts. Older bundles without source do not establish the size of a newly packed, source-bearing app.</p>
        <p>Runtime size varies by release, platform and packaging. Use the <a href="{facts["latest_release_url"]}">published release assets</a> for download sizes; measure installed size separately. No game-versus-editor or game-versus-chat-client size ratio is used here.</p>
      </section>

      <section class="prose rsec">
        <span class="eye">Reproduce the right thing</span>
        <h2>How to evaluate your application.</h2>
        <ol class="steps">
          <li>Choose equivalent user tasks and verify their outputs before timing.</li>
          <li>Record machine, OS, architecture, app and runtime versions, input digests, dependencies and permission settings.</li>
          <li>Separate full bundle, code payload, runtime download and installed footprint. Include the shared runtime when evaluating first install.</li>
          <li>Measure cold and warm runs separately. Retain samples, process-tree accounting and failure cases rather than reporting only the best run.</li>
          <li>Publish limits and reproduction steps beside any headline ratio.</li>
        </ol>
        <p>Repository inventory is not proof of current release compatibility. Consult <a href="/progress/">the inventory and test links</a>, <a href="/docs/limits.html">current limits</a> and <a href="/docs/porting.html">the porting guide</a> before choosing a workload.</p>
        <div class="pl">
          <a class="btn dark" href="/docs/quickstart.html">Build your first app</a>
          <a class="btn ghost" href="{facts["repository"]}">Source</a>
          <a class="btn link" href="/">Back to krate.tech {K.ARROW}</a>
        </div>
      </section>
    </div>
  </div>
</div>
</main>

{K.FOOTER}
{K.KIT_SCRIPT}
</body>
</html>
"""
    html = add_contents(html)
    args.output.write_text(html)
    print(f"wrote {args.output} ({len(inventory)} artifacts; scoped benchmark reused, no apps timed)")


if __name__ == "__main__":
    main()
