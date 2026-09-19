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

def add_contents(page):
    """Give every <h2> an id and put a contents list under the lede.

    This page is ten sections and, measured at 390px wide, 16 screens tall
    with no in-page links and nothing collapsible: on a phone the only way
    to reach the last section was to scroll past every other one, with no
    way to tell what was coming or how far in you were.

    Derived from the headings themselves rather than written by hand, so a
    renamed or reordered section cannot leave a contents list pointing at
    something that is no longer there.
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
        # The plain-text heading, for the link label.
        label = re.sub(r"<[^>]+>", "", inner)
        label = label.replace("&ndash;", "-").replace("&nbsp;", " ").replace("&amp;", "&")
        heads.append((ident, label.strip().rstrip(".")))
        return f'<h2 id="{ident}">{inner}</h2>'

    page = re.sub(r"<h2>(.*?)</h2>", tag, page, flags=re.S)
    if len(heads) < 3:
        return page

    items = "\n".join(
        f'        <li><a href="#{i}">{html_escape(t)}</a></li>' for i, t in heads
    )
    toc = f"""
    <nav class="toc" aria-label="What is in this report">
      <h2 class="toc-h">What is in here</h2>
      <ol>
{items}
      </ol>
    </nav>
"""
    # After the page's opening lede, before the first section.
    idx = page.find('<section')
    if idx == -1:
        return page
    page = page[:idx] + toc + page[idx:]

    css = """
    /* Contents for a long report. Ten sections and 16 screens on a phone
       is a blind scroll without it. Numbers come from the <ol>, so a
       section added or removed renumbers itself. */
    .toc { margin: 26px 0 8px; padding: 16px 18px; border: 1px solid var(--line-soft, rgba(255,255,255,0.1));
           border-radius: 10px; background: rgba(255,255,255,0.02); }
    .toc .toc-h { margin: 0 0 10px; font-size: 12px; text-transform: uppercase;
                  letter-spacing: 0.06em; color: var(--quiet, rgba(255,255,255,0.45)); font-weight: 500; }
    .toc ol { margin: 0; padding-left: 1.25em; }
    .toc li { margin: 0; }
    .toc a { display: block; padding: 9px 0; color: var(--text, #fff);
             text-decoration: none; line-height: 1.35; }
    .toc a:hover { text-decoration: underline; }
    /* The headings these link to sit under a sticky header on some
       screens; scroll-margin stops the jump hiding the title. */
    h2[id] { scroll-margin-top: 72px; }
    @media (max-width: 760px) {
      /* 9px top and bottom on a 1.35 line: a real tap target per row. */
      .toc { margin: 22px 0 6px; padding: 14px 16px; }
      .toc a { padding: 11px 0; }
    }
"""
    return page.replace("</style>", css + "  </style>", 1)


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
        f'<tr><td><a href="{source_url(r["path"])}"><code>{html_escape(r["path"])}</code></a></td>'
        f'<td class="num">{r["bundle_bytes"]:,}</td><td class="num">{r["code_bytes"]:,}</td>'
        f'<td>{"Yes" if r["source"] else "No"}</td></tr>' for r in inventory)
    analysis_url = source_url(claims[0]["source"])
    kit_url = source_url("evidence/benchmarks/marktext-vs-krate/README.md")
    audit_url = source_url(claims[0]["evidence_run"] + "/audit.txt")
    seal_url = source_url(claims[0]["seal"])
    revision = subprocess.run(["git", "rev-parse", "--short", "HEAD"], cwd=ROOT,
                              capture_output=True, text=True, check=True).stdout.strip()
    today = datetime.date.today().isoformat()
    html = f"""<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <meta name="theme-color" content="#0a0a0a" />
  <meta name="description" content="Krate benchmark methodology: a scoped MarkText notes comparison, complete bundle sizes, code payload and shared runtime costs." />
  <link rel="canonical" href="https://krate.tech/reports/" />
  <meta property="og:title" content="Krate: the measurements" />
  <meta property="og:description" content="A measured notes workload, with methods and limitations. Full application bundles and shared runtime costs are shown separately." />
  <meta property="og:type" content="website" />
  <meta property="og:url" content="https://krate.tech/reports/" />
  <meta property="og:image" content="https://krate.tech/og-v2.png" />
  <meta name="twitter:card" content="summary_large_image" />
  <title>Krate: the measurements</title>
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
  code {{ font-family: var(--mono); font-size: 0.86em; }}
  ul, ol {{ padding-left: 1.3em; color: var(--muted); }}
  li {{ margin: 4px 0; }}
  </style>
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
  <!-- This is the page a doubter is sent to, which is the worst one to look
       unhosted. Links are absolute because it is served from /reports/. -->
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

  <main class="report">
    <p class="eyebrow">Reports</p>
    <h1>Measurements you can inspect.</h1>
    <p class="lede">A notes workload compared on the same machine, plus an
      inventory of application files in this checkout. These are different
      kinds of evidence: neither establishes a performance result for every
      Krate app or for the latest release.</p>
    <p class="method">Page generated {today} from commit <code>{revision}</code>.
      Benchmark measured {html_escape(claims[0]["measured_on"])}.
      Building this page does not run a new benchmark.</p>

    <section>
      <p class="eyebrow">One matched workload</p>
      <h2>Krate notes and MarkText.</h2>
      <p>{html_escape(claims[0]["scope"])}. This does not establish full editor
        feature parity. Warm-open and memory rows refer to the same
        50,000-line document, not different document sizes.</p>
      <div class="table-scroll">
        <table>
          <thead><tr><th>Measurement</th><th>Krate</th><th>MarkText</th></tr></thead>
          <tbody>{benchmark_rows}</tbody>
        </table>
      </div>
      <p class="method">Warm open is time to a visible window, median of ten
        runs per app. Memory is settled footprint across the whole process
        tree. The historical app-bundle row excludes Krate's shared runtime and does
        not measure a current source-bearing bundle.</p>
      <p class="method">{html_escape(claims[0]["caveat"])}.
        That first-app ratio is the historical runtime-plus-payload accounting,
        not a current source-bearing bundle installation measurement.</p>
      <p>Lower numbers in these rows are useful evidence for this workload,
        not a universal efficiency claim. Application features, assets,
        rendering, host calls and data handling can change the result.
        Energy was not measured.</p>
      <p><a href="{analysis_url}">Read the run analysis</a> ·
        <a href="{kit_url}">Protocol and reproduction scripts</a> ·
        <a href="{audit_url}">Audit scope</a> ·
        <a href="{seal_url}">Run seal</a></p>
      <p class="method">The public checkout includes the protocol, analysis,
        machine summary and seal. It does not distribute every retained raw
        sample. A seal records file identity, not an independent review.
        Reproduction requires the documented apps, hardware and measurement
        setup; generating this website is not reproduction of the experiment.</p>
    </section>

    <section>
      <p class="eyebrow">What the recipient downloads</p>
      <h2>Bundle, code payload and runtime are different costs.</h2>
      <p>{html_escape(facts["bundle_accounting"])}</p>
      <p>{html_escape(facts["prerequisite"])}</p>
      <div class="table-scroll">
        <table>
          <thead><tr><th>Repository artifact</th><th>Full bundle bytes</th>
            <th>Code payload bytes</th><th>Carries source</th></tr></thead>
          <tbody>{inventory_rows}</tbody>
        </table>
      </div>
      <p class="method">{len(inventory)} artifacts in this checkout. Full
        bundle bytes are the actual archive length; code payload is the
        uncompressed <code>code.wasm</code> entry. Compression means payload
        bytes are not an additive part of the archive length. Equal app names
        on different shelves are retained as different artifacts. Older
        bundles without source do not establish the size of a newly packed,
        source-bearing app.</p>
      <p>Runtime size varies by release, platform and packaging. Use the
        <a href="{facts["latest_release_url"]}">published release assets</a>
        for download sizes; measure installed size separately.
        No game-versus-editor or game-versus-chat-client size ratio is used here.</p>
    </section>

    <section>
      <p class="eyebrow">Reproduce the right thing</p>
      <h2>How to evaluate your application.</h2>
      <ol>
        <li>Choose equivalent user tasks and verify their outputs before timing.</li>
        <li>Record machine, OS, architecture, app and runtime versions, input
          digests, dependencies and permission settings.</li>
        <li>Separate full bundle, code payload, runtime download and installed
          footprint. Include the shared runtime when evaluating first install.</li>
        <li>Measure cold and warm runs separately. Retain samples, process-tree
          accounting and failure cases rather than reporting only the best run.</li>
        <li>Publish limits and reproduction steps beside any headline ratio.</li>
      </ol>
      <p>Repository inventory is not proof of current release compatibility.
        Consult <a href="/progress/">the inventory and test links</a>,
        <a href="/docs/limits.html">current limits</a> and
        <a href="/docs/porting.html">the porting guide</a> before choosing a workload.</p>
    </section>
    <div class="page-links">
      <a href="/docs/quickstart.html">Build your first app</a>
      <a href="{facts["repository"]}">Source</a>
      <a href="/">Back to krate.tech</a>
    </div>
  </main>
  <footer class="subfoot">
    <div class="wrap subfoot-inner">
      <span>© 2026 Krate Labs</span>
      <span>
        <a href="/docs/">Docs</a>
        <a href="/progress/">Progress</a>
        <a href="https://github.com/incyashraj/krate">GitHub</a>
      </span>
    </div>
  </footer>
</body>
</html>
"""
    html = add_contents(html)
    args.output.write_text(html)
    print(f"wrote {args.output} ({len(inventory)} artifacts; scoped benchmark reused, no apps timed)")


if __name__ == "__main__":
    main()
