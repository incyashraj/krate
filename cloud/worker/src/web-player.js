// The browser Player's side of the hub (IC-956 Phase 1).
//
// A Krate app opens in a tab on the browser's own WebAssembly engine. Three
// things here make that possible:
//
// 1. The web pack, made once per release at publish: the judged component
//    run through js-component-bindgen (the engine inside `jco transpile`,
//    crates/web-transpile compiled to wasm like the validator), stored in R2
//    beside the bundle at web/<tag>/<hash>. Idempotent: the same bytes are
//    never transpiled twice, and a release published before this existed is
//    made on its first open.
// 2. GET /a/<hash>/web: the bundle and its web pack in one answer, behind
//    exactly the checks /a/<hash> applies (a takedown, an organisation's
//    private release asked of its store with the person's own session).
// 3. GET /run/<hash> and /player/*: the Player page, sent with the headers
//    that make a tab cross-origin isolated (COOP same-origin + COEP
//    require-corp, the pair Phase 0 measured working in Chromium and
//    WebKit) -- which the app's blocking worker needs for shared memory.

import * as transpilerImport from "./transpiler.wasm";

/// Which transpiler made a pack; part of its R2 key, so a pack made by
/// another version is never served as this one's. Must name the version
/// crates/web-transpile pins (TRANSPILER there).
export const WEB_PACK_TAG = "jcb-2.12.0";
export const WEB_PACK_TRANSPILER = "js-component-bindgen 2.12.0 (jco 1.35.0)";

/// A bundle larger than this is not unpacked in the Worker to make a pack;
/// such an app opens in Krate.
const MAX_PACK_SOURCE_BYTES = 16 * 1024 * 1024;

let transpilerExports = null;
function transpiler() {
  if (!transpilerExports) {
    transpilerExports =
      transpilerImport.default instanceof WebAssembly.Module
        ? new WebAssembly.Instance(transpilerImport.default, {}).exports
        : transpilerImport;
  }
  return transpilerExports;
}

/// The transpiler's answer for one component: `{ header, bytes }`, where
/// `bytes` is what is stored and served ([u32 header length][header][files]).
export function transpileComponent(component) {
  const t = transpiler();
  const input = t.krate_alloc(component.length);
  new Uint8Array(t.memory.buffer, input, component.length).set(component);
  const out = t.krate_transpile(input, component.length);
  const view = new DataView(t.memory.buffer);
  const length = view.getUint32(out, true);
  const bytes = new Uint8Array(t.memory.buffer, out + 4, length).slice();
  t.krate_free(out, 4 + length);
  t.krate_free(input, component.length);
  const hlen = new DataView(bytes.buffer).getUint32(0, true);
  const header = JSON.parse(new TextDecoder().decode(bytes.subarray(4, 4 + hlen)));
  return { header, bytes };
}

const packKey = (hash) => `web/${WEB_PACK_TAG}/${hash}`;

/// Make the web pack for `hash` if it does not exist yet. `judgedComponent`
/// is the validator's: the component exactly as judged, or the refusal.
/// Answers `{ ok: true, bytes }` or `{ ok: false, problem }`; never throws.
export async function ensureWebPack(env, hash, bundle, judgedComponent) {
  try {
    const existing = await env.BUNDLES.get(packKey(hash));
    if (existing) {
      const bytes = new Uint8Array(await existing.arrayBuffer());
      const failed = existing.customMetadata && existing.customMetadata.failed;
      return failed ? { ok: false, problem: failed } : { ok: true, bytes, cached: true };
    }
    if (bundle.length > MAX_PACK_SOURCE_BYTES) {
      return { ok: false, problem: "too big to open in a tab" };
    }
    const component = judgedComponent(bundle);
    if (!component.ok) return { ok: false, problem: component.problem };
    const { header, bytes } = transpileComponent(component.bytes);
    if (!header.ok) {
      // Remembered, so a component the transpiler refuses costs one try.
      await env.BUNDLES.put(packKey(hash), new Uint8Array(0), {
        customMetadata: { failed: String(header.problem || "the transpiler refused it").slice(0, 500) },
      });
      return { ok: false, problem: header.problem };
    }
    await env.BUNDLES.put(packKey(hash), bytes, {
      customMetadata: { transpiler: WEB_PACK_TRANSPILER, component: header.component_sha256 },
    });
    return { ok: true, bytes };
  } catch (err) {
    return { ok: false, problem: String((err && err.message) || err) };
  }
}

/// The headers a Player page and everything it loads carry: cross-origin
/// isolated (shared memory for the app's blocking worker), loadable by this
/// origin only, and scripts only from here and the Blob the glue becomes.
export const ISOLATED = {
  "cross-origin-opener-policy": "same-origin",
  "cross-origin-embedder-policy": "require-corp",
  "cross-origin-resource-policy": "same-origin",
  "content-security-policy":
    "default-src 'self'; script-src 'self' blob: 'wasm-unsafe-eval'; worker-src 'self' blob:; " +
    // ws: and wss: named: a bare * has not always covered them, and the
    // team data's live socket (org-data-live.js) is one.
    "connect-src * ws: wss:; img-src 'self' data: blob:; style-src 'self'; font-src 'self'; " +
    "frame-ancestors 'none'; base-uri 'none'; form-action 'self'",
  "x-content-type-options": "nosniff",
  "referrer-policy": "no-referrer",
};

const TYPES = {
  html: "text/html; charset=utf-8",
  js: "text/javascript; charset=utf-8",
  css: "text/css; charset=utf-8",
  wasm: "application/wasm",
  woff2: "font/woff2",
  png: "image/png",
  json: "application/json",
  svg: "image/svg+xml",
};

/// The files the Player page is made of, by name. Anything else under
/// /player/ is not found.
const PLAYER_FILES = new Set([
  "run.html",
  "player.js",
  "player.css",
  "app-worker.js",
  "hosts.js",
  "bridge.js",
  "ring.js",
  "org-data.js",
  "org-device.js",
  "krate_web.js",
  "krate_web_bg.wasm",
  "validator.wasm",
  "geist-var-latin.woff2",
  "krate-mark-3d-96.png",
  "krate-favicon.png",
  "krate-app-icon.png",
]);

async function asset(env, url, name) {
  if (!env.ASSETS || !PLAYER_FILES.has(name)) return null;
  const res = await env.ASSETS.fetch(new Request(new URL(`/${name}`, url)));
  if (!res || res.status !== 200) return null;
  const ext = name.slice(name.lastIndexOf(".") + 1);
  return new Response(res.body, {
    status: 200,
    headers: {
      "content-type": TYPES[ext] || "application/octet-stream",
      "cache-control": name === "run.html" ? "no-cache" : "public, max-age=300",
      ...ISOLATED,
    },
  });
}

/// The Player's addresses: one release (`/run/<hash>`), or an
/// organisation's app as it is now (`/run/org/<slug>/<app-id>`, which the
/// page resolves to the current release with the person's session -- the
/// address a phone keeps on its home screen, so an update never breaks it),
/// each with its web app manifest and icon.
export const RUN_PATH = /^\/run\/(?:([0-9a-f]{64})|org\/([a-z][a-z0-9-]{1,38})\/([a-z0-9][a-z0-9._-]{0,127}))(?:\/(app\.webmanifest|icon\.svg))?\/?$/;

/// What a home screen calls an app: the name the page judged, trimmed, with
/// nothing that is not printable.
function appName(raw) {
  const name = String(raw || "").replace(/[\u0000-\u001f\u007f-\u009f<>]/g, "").replace(/\s+/g, " ").trim().slice(0, 45);
  return name || "Krate app";
}

/// The icon's colour: one of the site's own, chosen by the address, so each
/// app keeps its colour and two apps rarely share one.
const ICON_COLOURS = ["#3d6df0", "#22a35a", "#b7791f", "#7c4dff", "#e5484d", "#0f9d9a", "#d6409f", "#52525b"];
function colourOf(text) {
  let h = 5381;
  for (const c of text) h = ((h * 33) ^ c.charCodeAt(0)) >>> 0;
  return ICON_COLOURS[h % ICON_COLOURS.length];
}

/// The letter on the icon: the name's first letter or digit.
function initialOf(name) {
  const m = /[\p{L}\p{N}]/u.exec(name);
  return m ? m[0].toLocaleUpperCase() : "K";
}

const escapeXml = (t) => t.replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&apos;" })[c]);

/// GET /run/<...>/app.webmanifest?name=<name>: what lets a phone (or a
/// computer's browser) add the app to its home screen as itself -- its name,
/// its icon, opening on its own without the browser's bar.
function manifestFor(base, name) {
  const body = {
    id: base,
    name,
    short_name: name.length > 12 ? name.split(" ")[0].slice(0, 12) : name,
    description: `${name}, a Krate app`,
    start_url: base,
    // The whole Player, so the address a release opens from is inside it too.
    scope: "/run/",
    display: "standalone",
    background_color: "#fbfbfd",
    theme_color: "#fbfbfd",
    icons: [
      { src: `${base}/icon.svg?name=${encodeURIComponent(name)}`, sizes: "any", type: "image/svg+xml", purpose: "any" },
      { src: "/player/krate-app-icon.png", sizes: "1024x1024", type: "image/png", purpose: "any" },
    ],
  };
  return new Response(JSON.stringify(body), {
    headers: { "content-type": "application/manifest+json; charset=utf-8", "cache-control": "public, max-age=300", ...ISOLATED },
  });
}

/// GET /run/<...>/icon.svg?name=<name>: the app's initial on its colour.
function iconFor(base, name) {
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512"><rect width="512" height="512" fill="${colourOf(base)}"/>` +
    `<text x="256" y="256" dy=".35em" text-anchor="middle" font-family="Geist, -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif" font-size="280" font-weight="600" fill="#fff">${escapeXml(initialOf(name))}</text></svg>`;
  return new Response(svg, {
    headers: { "content-type": "image/svg+xml; charset=utf-8", "cache-control": "public, max-age=86400", ...ISOLATED },
  });
}

/// GET /run/<hash> or /run/org/<slug>/<app-id>: the Player page for one
/// app. The page itself decides nothing about the app until it has fetched
/// and judged it.
export async function playerPage(request, url, env) {
  const m = RUN_PATH.exec(url.pathname);
  if (!m) return null;
  const base = m[1] ? `/run/${m[1]}` : `/run/org/${m[2]}/${m[3]}`;
  if (m[4] === "app.webmanifest") return manifestFor(base, appName(url.searchParams.get("name")));
  if (m[4] === "icon.svg") return iconFor(base, appName(url.searchParams.get("name")));
  const page = await asset(env, url, "run.html");
  if (page) return page;
  return new Response("The Player is not installed on this hub yet: run scripts/build-web-player.sh before deploying.", {
    status: 503,
    headers: { "content-type": "text/plain; charset=utf-8", ...ISOLATED },
  });
}

/// GET /player/<file>.
export async function playerAsset(request, url, env) {
  const name = url.pathname.slice("/player/".length);
  const res = await asset(env, url, name);
  return res || new Response("not found", { status: 404, headers: { "content-type": "text/plain", ...ISOLATED } });
}

/// GET /validator.wasm: the same module, for krate.tech's pages, which
/// load it cross-origin (a tab signing as an organisation's device,
/// IC-958). Not isolated, and readable from any origin.
export async function validatorFile(request, url, env) {
  const res = env.ASSETS && (await env.ASSETS.fetch(new Request(new URL("/validator.wasm", url))));
  if (!res || res.status !== 200) {
    return new Response("not found", { status: 404, headers: { "content-type": "text/plain", "access-control-allow-origin": "*" } });
  }
  return new Response(request.method === "HEAD" ? null : res.body, {
    status: 200,
    headers: {
      "content-type": "application/wasm",
      "access-control-allow-origin": "*",
      "cache-control": "no-cache",
      "cross-origin-resource-policy": "cross-origin",
    },
  });
}

/// GET /a/<hash>/web. `access(object)` applies /a/<hash>'s own checks and
/// answers null (go ahead) or the refusal Response.
export async function webPackRoute(env, hash, { lookup, access, judgedComponent }) {
  const found = await lookup(hash);
  if (found.response) return found.response;
  const { object, privateOrg } = found;
  if (privateOrg) {
    const refused = await access(object, privateOrg);
    if (refused) return refused;
  }
  const noStore = { "cache-control": "private, no-store", vary: "authorization" };
  const cache = privateOrg ? noStore : { "cache-control": "public, max-age=300" };
  if (object.size > MAX_PACK_SOURCE_BYTES) {
    try { if (object.body && object.body.cancel) await object.body.cancel(); } catch (_) {}
    return webJson({ error: "no-tab", message: "This app is too big to open in a tab. Open it in Krate on your computer." }, 422, cache);
  }
  const bundle = new Uint8Array(await object.arrayBuffer());
  const pack = await ensureWebPack(env, hash, bundle, judgedComponent);
  if (!pack.ok) {
    return webJson({ error: "no-tab", message: "This app opens in Krate, not in a tab yet.", detail: pack.problem }, 422, cache);
  }
  const body = new Uint8Array(4 + bundle.length + pack.bytes.length);
  new DataView(body.buffer).setUint32(0, bundle.length, true);
  body.set(bundle, 4);
  body.set(pack.bytes, 4 + bundle.length);
  return new Response(body, {
    headers: {
      "content-type": "application/octet-stream",
      "content-length": String(body.length),
      "cross-origin-resource-policy": "same-origin",
      "x-krate-web-pack": WEB_PACK_TAG,
      ...cache,
    },
  });
}

function webJson(value, status, extra) {
  return new Response(JSON.stringify(value), {
    status,
    headers: { "content-type": "application/json", "cross-origin-resource-policy": "same-origin", ...(extra || {}) },
  });
}
