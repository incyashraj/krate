// krate.tech's code (IC-997). Every other byte is a plain static asset.
//
// 1. Markdown for agents: a request whose Accept header asks for
//    text/markdown gets the page's markdown (written next to each page at
//    build by scripts/build-agent-files.py), so an AI reads the page's words
//    rather than scraping its layout. Browsers never send that header and get
//    HTML exactly as before. The home page's markdown is llms.txt.
// 2. /page.html is served as itself, as GitHub Pages served it. Cloudflare's
//    static assets answer it with a redirect to /page, and every page names
//    its .html address as canonical.
//
// 3. Krate for Claude Desktop (IC-1005): krate.tech/claude/krate-mac.mcpb
//    and krate-windows.mcpb redirect to the hub's stable address, which
//    sends the newest release's file, or says it is coming.
//
// Only page addresses and those two reach this code (run_worker_first in
// wrangler.toml).

/// The hub address a /claude/<file>.mcpb download goes to, or null.
export function desktopDownload(pathname) {
  const m = /^\/claude\/(krate-(?:mac|windows)\.mcpb)$/.exec(pathname);
  return m ? `https://hub.krate.tech/claude/${m[1]}` : null;
}

/// The markdown file beside a page address, or null when it is not a page.
export function markdownPath(pathname) {
  if (pathname.endsWith("/")) return pathname + "index.md";
  if (pathname.endsWith(".html")) return pathname.slice(0, -".html".length) + ".md";
  return null;
}

/// Whether the client asked for markdown in preference to HTML.
export function wantsMarkdown(accept) {
  const types = String(accept || "")
    .split(",")
    .map((part) => {
      const [type, ...params] = part.trim().toLowerCase().split(";");
      const q = params.map((p) => p.trim()).find((p) => p.startsWith("q="));
      return { type: type.trim(), q: q ? Number(q.slice(2)) : 1 };
    });
  const md = types.find((t) => t.type === "text/markdown");
  if (!md || !(md.q > 0)) return false;
  const html = types.find((t) => t.type === "text/html");
  return !html || md.q >= html.q;
}

function withVary(response) {
  const out = new Response(response.body, response);
  const vary = out.headers.get("vary");
  if (!vary || !/\baccept\b/i.test(vary)) out.headers.set("vary", vary ? `${vary}, Accept` : "Accept");
  return out;
}

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    const download = desktopDownload(url.pathname);
    if (download) {
      return new Response(null, { status: 302, headers: { location: download, "cache-control": "public, max-age=300" } });
    }
    if ((request.method === "GET" || request.method === "HEAD") && wantsMarkdown(request.headers.get("accept"))) {
      const path = markdownPath(url.pathname);
      if (path) {
        const md = await env.ASSETS.fetch(new Request(new URL(path, url), { method: request.method }));
        if (md.status === 200) {
          const body = request.method === "HEAD" ? null : await md.text();
          const headers = new Headers({
            "content-type": "text/markdown; charset=utf-8",
            vary: "Accept",
            "cache-control": md.headers.get("cache-control") || "public, max-age=0, must-revalidate",
            "access-control-allow-origin": "*",
            // The page this text represents, for a reader that wants to cite it.
            link: `<${url.origin}${url.pathname}>; rel="canonical"`,
          });
          // A rough size for an agent budgeting its context (Cloudflare's header).
          if (body) headers.set("x-markdown-tokens", String(Math.ceil(body.length / 4)));
          return new Response(body, { status: 200, headers });
        }
      }
    }
    if (url.pathname.endsWith(".html") && !url.pathname.endsWith("/index.html")) {
      const bare = new URL(url);
      bare.pathname = url.pathname.slice(0, -".html".length);
      const page = await env.ASSETS.fetch(new Request(bare, request));
      if (page.status === 200) return withVary(page);
    }
    return withVary(await env.ASSETS.fetch(request));
  },
};
