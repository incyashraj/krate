// Studio's interface, updated without a release (IC-998).
//
// The hub only holds the bundle; it does not vouch for it. Every Studio
// checks the signature itself against the key compiled into it
// (studio/src/live_ui.rs), so a hub that served something else -- or an
// admin token that leaked -- still cannot change what a Studio shows.
//
//   GET /studio-ui/latest     {serial, commit, size} -- one small read per Studio start
//   GET /studio-ui/bundle     the bundle's exact bytes
//   GET /studio-ui/bundle.sig its Ed25519 signature, hex
//   PUT /admin/studio-ui      {bundle, sig} from scripts/ship-studio-ui.sh (admins only)
//
// KV, not R2: one ~3 MB value well inside KV's 25 MiB, and nothing for the
// bucket's orphan scrub to wonder about. Three writes per ship.

const MAX_BUNDLE = 20 * 1024 * 1024;

export async function studioUiRoute(request, url, env, { isAdmin, json, text }) {
  const path = url.pathname;
  if (request.method === "GET" && path === "/studio-ui/latest") {
    const latest = await env.APPS.get("studio-ui:latest");
    return json(latest ? JSON.parse(latest) : { serial: 0 });
  }
  if (request.method === "GET" && (path === "/studio-ui/bundle" || path === "/studio-ui/bundle.sig")) {
    const body = await env.APPS.get(path.endsWith(".sig") ? "studio-ui:sig" : "studio-ui:bundle");
    if (!body) return text("no interface update has been shipped", 404);
    return new Response(body, {
      headers: { "content-type": path.endsWith(".sig") ? "text/plain" : "application/json", "cache-control": "no-store" },
    });
  }
  if (request.method === "PUT" && path === "/admin/studio-ui") {
    if (!(await isAdmin(request, env))) return text("not found", 404);
    const ask = await request.json().catch(() => null);
    const bundle = ask && typeof ask.bundle === "string" ? ask.bundle : "";
    const sig = ask && typeof ask.sig === "string" ? ask.sig.trim() : "";
    if (!bundle || bundle.length > MAX_BUNDLE) return text("send {bundle, sig}: the bundle's exact text and its signature", 400);
    if (!/^[0-9a-f]{128}$/.test(sig)) return text("the signature is 64 bytes of hex", 400);
    let parsed;
    try {
      parsed = JSON.parse(bundle);
    } catch {
      return text("the bundle is not JSON", 400);
    }
    if (parsed.format !== 1 || !Number.isFinite(parsed.serial) || !parsed.files || !parsed.files["index.html"]) {
      return text("not an interface bundle (format 1, serial, files with index.html)", 400);
    }
    const prior = JSON.parse((await env.APPS.get("studio-ui:latest")) || "null");
    if (prior && prior.serial >= parsed.serial) {
      return text(`the hub already holds a newer or equal interface (${prior.serial}); ship a newer commit`, 409);
    }
    await env.APPS.put("studio-ui:bundle", bundle);
    await env.APPS.put("studio-ui:sig", sig);
    const latest = { serial: parsed.serial, commit: String(parsed.commit || ""), size: bundle.length, at: Math.floor(Date.now() / 1000) };
    await env.APPS.put("studio-ui:latest", JSON.stringify(latest));
    return json({ ok: true, ...latest });
  }
  return null;
}
