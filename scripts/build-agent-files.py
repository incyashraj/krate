#!/usr/bin/env python3
"""The files AI agents look for on krate.tech, generated into the built site.

  scripts/build-agent-files.py _site        write them
  scripts/build-agent-files.py --self-test  check the generator itself

Each file follows the published standard an agent-readiness scanner checks
(isitagentready.com/llms-full.txt), and each is generated from the one
place that already holds its facts, so none can drift from the truth:

  /.well-known/mcp/server-card.json   MCP Server Card (SEP-2127), from the MCP
                                      registry entry and the hub's protocol list
  /.well-known/ard.json               Agentic Resource Discovery (v0.91) entries,
  /.well-known/ai-catalog.json        and the same as an AI Catalog (its predecessor)
  /.well-known/api-catalog            API Catalog (RFC 9727), application/linkset+json
  /.well-known/agent-skills/...       Agent Skills index (v0.2.0) and SKILL.md,
                                      its digest computed from the bytes served
  <every page>.md                     the page as markdown, for an agent that
                                      asks for text/markdown (cloud/site/site.js)
"""
import hashlib
import html
import json
import re
import sys
import tempfile
from html.parser import HTMLParser
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
ORIGIN = "https://krate.tech"
sys.path.insert(0, str(ROOT / "scripts"))
import public_facts  # noqa: E402


# ------------------------------------------------------------------ the MCP card

def mcp_versions(root=ROOT):
    """The protocol versions the hub's MCP server answers (cloud/worker/src/mcp.js)."""
    src = (root / "cloud/worker/src/mcp.js").read_text()
    match = re.search(r"export const MCP_VERSIONS = \[([^\]]+)\]", src)
    if not match:
        raise ValueError("mcp.js no longer exports MCP_VERSIONS")
    return re.findall(r'"(\d{4}-\d{2}-\d{2})"', match[1])


def server_card(root=ROOT):
    entry = json.loads((root / "cloud/worker/mcp-registry/server.json").read_text())
    remotes = []
    for remote in entry["remotes"]:
        remotes.append({**remote, "supportedProtocolVersions": mcp_versions(root)})
    card = {
        "$schema": "https://static.modelcontextprotocol.io/schemas/v1/server-card.schema.json",
        "name": entry["name"],
        "title": entry["title"],
        "description": entry["description"],
        "version": entry["version"],
        "websiteUrl": entry["websiteUrl"],
        "repository": entry["repository"],
        "icons": entry["icons"],
        "remotes": remotes,
    }
    if not re.fullmatch(r"[a-zA-Z0-9.-]+/[a-zA-Z0-9._-]+", card["name"]):
        raise ValueError("an MCP server card name is reverse-DNS with one slash")
    return card


# --------------------------------------------------------------- the agent skill

SKILL_SOURCE = "docs/agents/krate-apps/SKILL.md"
SKILL_PATH = ".well-known/agent-skills/krate-apps/SKILL.md"


def skill_text(root=ROOT):
    """SKILL.md with the release and the comparison filled in from the facts."""
    facts = public_facts.load_facts(root)
    version, _ = public_facts.released_version(root)
    claims = public_facts.benchmark_claims(root)
    text = (root / SKILL_SOURCE).read_text()
    fills = {
        "release": version,
        "when_to_choose": facts["when_to_choose"],
        "compared": "\n".join("- " + line for line in facts["compared"]),
        "analysis_url": public_facts.source_url(claims[0]["source"]),
    }
    for key, value in fills.items():
        text = text.replace("{{" + key + "}}", value)
    left = re.findall(r"\{\{[a-z_]+\}\}", text)
    if left:
        raise ValueError(f"SKILL.md names facts nobody fills: {left}")
    front = re.match(r"---\nname: ([a-z0-9-]+)\ndescription: (.+?)\n---\n", text, re.S)
    if not front:
        raise ValueError("SKILL.md starts with name and description frontmatter")
    name, description = front[1], front[2].strip()
    if not re.fullmatch(r"[a-z0-9]+(-[a-z0-9]+)*", name) or len(name) > 64:
        raise ValueError("a skill name is 1-64 lowercase letters, digits and single hyphens")
    if len(description) > 1024:
        raise ValueError("a skill description is at most 1024 characters")
    return name, description, text


def skills_index(name, description, body):
    digest = hashlib.sha256(body.encode()).hexdigest()
    return {
        "$schema": "https://schemas.agentskills.io/discovery/0.2.0/schema.json",
        "skills": [{
            "name": name,
            "type": "skill-md",
            "description": description,
            "url": f"{ORIGIN}/{SKILL_PATH}",
            "digest": f"sha256:{digest}",
        }],
    }


# ------------------------------------------------------------ discovery catalogs

def catalog(skill_description):
    entries = [
        {
            "identifier": "urn:air:krate.tech:mcp:krate",
            "displayName": "Krate",
            "description": "Make a real app by asking: Krate builds and checks it, and its link opens in any browser, "
                           "phones included. The same file runs natively on macOS, Windows and Linux.",
            "type": "application/mcp-server-card+json",
            "url": f"{ORIGIN}/.well-known/mcp/server-card.json",
            "representativeQueries": [
                "make me an app that works on my phone and my computer",
                "build a cross-platform desktop app that ships as one file",
                "share an app I made with AI as a link",
                "a lighter alternative to Electron for an internal tool",
            ],
        },
        {
            "identifier": "urn:air:krate.tech:skills:krate-apps",
            "displayName": "Krate apps",
            "description": skill_description,
            "type": "application/agent-skills+md",
            "url": f"{ORIGIN}/{SKILL_PATH}",
            "representativeQueries": [
                "how do I make an app with Krate",
                "write a Krate app and check it",
                "when should I use Krate instead of Electron or Tauri",
            ],
        },
        {
            "identifier": "urn:air:krate.tech:docs:facts",
            "displayName": "Krate facts for AI readers",
            "description": "What Krate is, when to choose it, how it compares with Electron, Tauri and web apps, "
                           "and its recorded measurements.",
            "type": "text/markdown",
            "url": f"{ORIGIN}/llms.txt",
            "representativeQueries": [
                "what is Krate",
                "Krate vs Electron memory and start time",
                "how does a Krate app run on every platform from one file",
            ],
        },
    ]
    return {
        "specVersion": "1.0",
        "host": {
            "displayName": "Krate",
            "documentationUrl": f"{ORIGIN}/docs/",
            "logoUrl": f"{ORIGIN}/krate-favicon.png",
        },
        "entries": entries,
    }


def api_catalog():
    """RFC 9727: the APIs Krate offers agents, each with its description, docs and status."""
    return {
        "linkset": [{
            "anchor": "https://hub.krate.tech/mcp",
            "service-desc": [{"href": f"{ORIGIN}/.well-known/mcp/server-card.json", "type": "application/json"}],
            "service-doc": [{"href": f"{ORIGIN}/docs/pages/make-an-app-with-claude.html", "type": "text/html"}],
            "status": [{"href": "https://hub.krate.tech/health", "type": "text/plain"}],
        }]
    }


# ------------------------------------------------------------- pages as markdown

SKIP_TAGS = {"script", "style", "noscript", "svg", "template", "header", "nav", "footer", "button", "form",
             "dialog", "select", "iframe", "canvas", "video", "audio"}
BLOCK = {"p", "div", "section", "article", "main", "aside", "figure", "figcaption", "details", "summary",
         "table", "tr", "dl", "dt", "dd", "blockquote", "ul", "ol", "li", "pre", "h1", "h2", "h3", "h4",
         "h5", "h6", "hr", "br"}


class Markdown(HTMLParser):
    """The readable content of a page as markdown: its <main> when it has one,
    without navigation, scripts or anything a page hides from people."""

    def __init__(self, base):
        super().__init__(convert_charrefs=True)
        self.base = base
        self.out, self.stack, self.skip = [], [], 0
        self.in_main = 0
        self.saw_main = False
        self.lists, self.href, self.link_text, self.pre = [], None, [], 0
        self.title, self.in_title = "", False
        self.row = None

    def emit(self, text):
        if self.skip:
            return
        if self.href is not None:
            self.link_text.append(text)
        else:
            self.out.append(text)

    def handle_starttag(self, tag, attrs):
        a = dict(attrs)
        if tag == "title":
            self.in_title = True
        if tag == "main":
            self.in_main += 1
            self.saw_main = True
        hidden = "hidden" in a or a.get("aria-hidden") == "true" or "hidden" in (a.get("class") or "").split()
        if tag in SKIP_TAGS or hidden:
            if tag not in {"br", "hr", "img", "input", "meta", "link"}:
                self.skip += 1
                self.stack.append(("skip", tag))
            return
        self.stack.append(("keep", tag))
        if tag in {"h1", "h2", "h3", "h4", "h5", "h6"}:
            self.emit("\n\n" + "#" * int(tag[1]) + " ")
        elif tag == "p":
            self.emit("\n\n")
        elif tag in {"ul", "ol"}:
            self.lists.append([tag, 0])
            self.emit("\n")
        elif tag == "li":
            depth = max(len(self.lists) - 1, 0)
            kind = self.lists[-1] if self.lists else ["ul", 0]
            kind[1] += 1
            marker = f"{kind[1]}." if kind[0] == "ol" else "-"
            self.emit("\n" + "  " * depth + marker + " ")
        elif tag == "pre":
            self.pre += 1
            self.emit("\n\n```\n")
        elif tag == "code" and not self.pre:
            self.emit("`")
        elif tag in {"strong", "b"}:
            self.emit("**")
        elif tag in {"em", "i"}:
            self.emit("*")
        elif tag == "br":
            self.emit("  \n")
        elif tag == "hr":
            self.emit("\n\n---\n\n")
        elif tag == "blockquote":
            self.emit("\n\n> ")
        elif tag == "a" and a.get("href") and not a["href"].startswith(("javascript:", "#")):
            href = a["href"]
            if href.startswith("/"):
                href = ORIGIN + href
            self.href, self.link_text = href, []
        elif tag == "img" and a.get("alt"):
            self.emit(f" {a['alt']} ")
        elif tag == "tr":
            self.row = []
        elif tag in {"td", "th"} and self.row is not None:
            self.row.append("")
        elif tag in BLOCK:
            self.emit("\n")

    def handle_endtag(self, tag):
        if tag == "title":
            self.in_title = False
        while self.stack:
            kind, open_tag = self.stack.pop()
            if kind == "skip":
                self.skip -= 1
            if open_tag == tag:
                break
        else:
            return
        if kind == "skip":
            return
        if tag == "main":
            self.in_main -= 1
        if tag in {"ul", "ol"} and self.lists:
            self.lists.pop()
            self.emit("\n")
        elif tag == "pre":
            self.pre -= 1
            self.emit("\n```\n\n")
        elif tag == "code" and not self.pre:
            self.emit("`")
        elif tag in {"strong", "b"}:
            self.emit("**")
        elif tag in {"em", "i"}:
            self.emit("*")
        elif tag == "a" and self.href is not None:
            text = " ".join("".join(self.link_text).split())
            href, self.href = self.href, None
            self.emit(f"[{text}]({href})" if text else "")
        elif tag == "tr" and self.row is not None:
            cells = [" ".join(c.split()) for c in self.row]
            self.row = None
            self.emit("\n| " + " | ".join(cells) + " |")
        elif tag in BLOCK:
            self.emit("\n")

    def handle_data(self, data):
        if self.in_title:
            self.title += data
            return
        if self.row is not None and self.row and not self.skip:
            self.row[-1] += data
            return
        if self.pre:
            self.emit(data)
        else:
            self.emit(re.sub(r"\s+", " ", data))


def page_markdown(html_text, url):
    """The page as markdown, from <main> when it has one; None for a page with no reading in it."""
    main = re.search(r"<main\b[^>]*>(.*)</main>", html_text, re.S | re.I)
    body = main[1] if main else (re.search(r"<body\b[^>]*>(.*)</body>", html_text, re.S | re.I) or [None, ""])[1]
    title = re.search(r"<title>(.*?)</title>", html_text, re.S | re.I)
    parser = Markdown(url)
    parser.feed(body)
    text = "".join(parser.out)
    # Bold and italic around nothing (an icon, an empty span) leave bare markers.
    for _ in range(3):
        text = re.sub(r"\*\*\s*\*\*", "", text)
    text = re.sub(r"(?m)^\s*\*{1,4}\s*$", "", text)
    text = re.sub(r"[ \t]+\n", "\n", text)
    text = re.sub(r"\n{3,}", "\n\n", text).strip()
    # Tables: a header separator after the first row of each run of rows.
    text = re.sub(r"((?:^\| .*\|\n?)+)", _table, text, flags=re.M)
    if len(text) < 200:
        return None
    head = html.unescape(title[1]).strip() if title else ""
    lead = f"# {head}\n\n" if head and not text.startswith("# ") else ""
    return f"{lead}{text}\n\nSource: {url}\n"


def _table(match):
    rows = match[1].rstrip("\n").split("\n")
    if len(rows) < 2:
        return match[1]
    cols = rows[0].count("|") - 1
    return "\n".join([rows[0], "|" + " --- |" * cols] + rows[1:]) + "\n"


def write_markdown_pages(site):
    written = 0
    for page in sorted(site.rglob("*.html")):
        rel = page.relative_to(site).as_posix()
        # Studio's own app, and pages kept from crawlers, have no reading to offer.
        if rel.startswith(("app/", "a/", "run/")) or rel.endswith("404.html") or rel == "docs/print.html":
            continue
        text = page.read_text(errors="replace")
        if re.search(r'<meta[^>]+name="robots"[^>]+noindex', text, re.I):
            continue
        if rel.endswith("index.html"):
            url = f"{ORIGIN}/{rel[:-len('index.html')]}"
        else:
            url = f"{ORIGIN}/{rel}"
        # The home page is a picture of the product; an agent asking for it as
        # text gets the curated description written for AI readers instead.
        if rel == "index.html" and (site / "llms.txt").is_file():
            page.with_suffix(".md").write_text((site / "llms.txt").read_text() + f"\nSource: {ORIGIN}/\n")
            written += 1
            continue
        md = page_markdown(text, url)
        if md:
            page.with_suffix(".md").write_text(md)
            written += 1
    return written


# --------------------------------------------------------------------- the build

def write_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, ensure_ascii=False) + "\n")


def build(site, root=ROOT):
    site = Path(site)
    write_json(site / ".well-known/mcp/server-card.json", server_card(root))
    name, description, skill = skill_text(root)
    (site / SKILL_PATH).parent.mkdir(parents=True, exist_ok=True)
    (site / SKILL_PATH).write_text(skill)
    write_json(site / ".well-known/agent-skills/index.json", skills_index(name, description, skill))
    cat = catalog(description)
    write_json(site / ".well-known/ard.json", cat)
    write_json(site / ".well-known/ai-catalog.json", cat)
    write_json(site / ".well-known/api-catalog", api_catalog())
    pages = write_markdown_pages(site)
    print(f"agent files: MCP server card, ARD and AI catalog, API catalog, the {name} skill, {pages} pages as markdown")


def self_test():
    card = server_card()
    assert card["name"] == "tech.krate/krate" and card["remotes"][0]["url"] == "https://hub.krate.tech/mcp"
    assert "2025-06-18" in card["remotes"][0]["supportedProtocolVersions"]
    name, description, skill = skill_text()
    version, _ = public_facts.released_version()
    assert name == "krate-apps" and version in skill and "{{" not in skill
    index = skills_index(name, description, skill)
    assert index["skills"][0]["digest"] == "sha256:" + hashlib.sha256(skill.encode()).hexdigest()
    # The digest is of the bytes as served: a skill changed after indexing fails it.
    assert index["skills"][0]["digest"] != "sha256:" + hashlib.sha256((skill + " ").encode()).hexdigest()
    cat = catalog(description)
    for entry in cat["entries"]:
        assert re.fullmatch(r"urn:air:krate\.tech:[a-z-]+:[a-z-]+", entry["identifier"])
        assert entry["displayName"] and entry["type"] and entry["url"].startswith(ORIGIN)
        assert 2 <= len(entry["representativeQueries"]) <= 5
    assert api_catalog()["linkset"][0]["service-desc"][0]["href"].endswith("server-card.json")
    page = ("<html><head><title>Why Krate</title></head><body><header><nav>Menu</nav></header><main>"
            "<h1>Why Krate</h1><p>One <b>small</b> file runs on <a href=\"/download/\">every</a> computer.</p>"
            "<ul><li>Fast</li><li>Safe</li></ul><script>var x = 1</script><div hidden>secret</div>"
            "<table><tr><th>App</th><th>Memory</th></tr><tr><td>Krate</td><td>178.5 MiB</td></tr></table>"
            "<pre><code>krate run app.krate</code></pre>" + "<p>More words here.</p>" * 12 +
            "</main><footer>Footer</footer></body></html>")
    md = page_markdown(page, "https://krate.tech/why-krate.html")
    assert md.startswith("# Why Krate"), md[:80]
    assert "One **small** file runs on [every](https://krate.tech/download/) computer." in md, md
    assert "- Fast" in md and "- Safe" in md
    assert "Menu" not in md and "Footer" not in md and "var x" not in md and "secret" not in md
    assert "| App | Memory |" in md and "| --- | --- |" in md and "| Krate | 178.5 MiB |" in md, md
    assert "```\nkrate run app.krate\n```" in md, md
    assert md.rstrip().endswith("Source: https://krate.tech/why-krate.html")
    assert page_markdown("<main><p>tiny</p></main>", "x") is None, "an empty page gets no markdown"
    with tempfile.TemporaryDirectory() as tmp:
        site = Path(tmp)
        (site / "faq").mkdir()
        (site / "faq/index.html").write_text(page)
        (site / "app").mkdir()
        (site / "app/index.html").write_text(page)
        build(site)
        assert (site / "faq/index.md").is_file() and not (site / "app/index.md").exists()
        served = (site / SKILL_PATH).read_bytes()
        idx = json.loads((site / ".well-known/agent-skills/index.json").read_text())
        assert idx["skills"][0]["digest"] == "sha256:" + hashlib.sha256(served).hexdigest()
        assert json.loads((site / ".well-known/api-catalog").read_text())["linkset"]
    print("ok  build-agent-files self-test")


if __name__ == "__main__":
    if sys.argv[1:] == ["--self-test"]:
        self_test()
    elif len(sys.argv) == 2:
        build(sys.argv[1])
    else:
        sys.exit("usage: build-agent-files.py <site dir> | --self-test")
