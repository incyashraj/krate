// krate.tech's one bit of code (IC-997): an address ending .html is
// served as itself, as GitHub Pages served it. Cloudflare's static assets
// answer /page.html with a redirect to /page, and every page names its
// .html address as canonical, so search engines would be told both. Only
// .html requests reach this code; every other file is a plain asset.
export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    if (url.pathname.endsWith(".html") && !url.pathname.endsWith("/index.html")) {
      const bare = new URL(url);
      bare.pathname = url.pathname.slice(0, -".html".length);
      const page = await env.ASSETS.fetch(new Request(bare, request));
      if (page.status === 200) return page;
    }
    return env.ASSETS.fetch(request);
  },
};
