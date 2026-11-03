// One app, many versions (IC-976).
//
// A publish is one content-addressed release. An app LINE is what a person
// thinks of as "the app": every release one owner publishes under one
// manifest app id, numbered v1, v2, ... in the order they arrived, with one
// of them current. The line has a link of its own, /a/v<11 hex>, which
// always opens the current version -- so a link sent at v1 opens v3 after
// two updates -- while each version's own link (/a/<hash prefix>) keeps
// opening exactly that build.
//
// Keys: line:<id> = { id, owner, app_id, name, current, versions: [...] }.
// One write per publish. Which line and version a build is rides on its own
// listing row (app:<hash> .line, .version_n), and an owner's line for an app
// is found from (owner, app id) itself, so neither needs a key of its own:
// the free plan's KV writes are counted (about 1,000 a day).
//
// The id is derived from (owner, app id), so the same app always lands on
// the same line and another owner's app with the same id is another line.
// It starts with "v", which no hex alias can, so the two link kinds never
// collide.

export const LINE_ID = /^v[0-9a-f]{11}$/;
/** A version list is bounded; the oldest fall off the record (their builds stay). */
const MAX_VERSIONS = 200;

async function sha256Hex(text) {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text));
  return [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, "0")).join("");
}

export async function lineIdOf(owner, appId) {
  return "v" + (await sha256Hex(`${owner}\n${appId}`)).slice(0, 11);
}

export async function readLine(env, id) {
  if (!LINE_ID.test(String(id || ""))) return null;
  try {
    return JSON.parse((await env.APPS.get(`line:${id}`)) || "null");
  } catch (_) {
    return null;
  }
}

/// The build a line link opens now, or null.
export async function currentHash(env, id) {
  const line = await readLine(env, id);
  if (!line) return null;
  const v = line.versions.find((x) => x.n === line.current);
  return v ? v.hash : null;
}

/// Which line and version a build is, with the line's current version, or null.
export async function versionOf(env, hash) {
  let row;
  try {
    row = JSON.parse((await env.APPS.get(`app:${hash}`)) || "null");
  } catch (_) {
    row = null;
  }
  if (!row || !row.line) return null;
  const line = await readLine(env, row.line);
  if (!line) return null;
  return { line: line.id, n: row.version_n, current: line.current, name: line.name, versions: line.versions.length };
}

/// Record a publish on its line: the next version, or nothing new when these
/// bytes are already a version of it. Returns { id, n, line, added }.
export async function advanceLine(env, { owner, appId, name, hash, version, size }) {
  const id = await lineIdOf(owner, appId);
  const now = Math.floor(Date.now() / 1000);
  const line = (await readLine(env, id)) || { id, owner, app_id: appId, name, current: 0, versions: [], created: now };
  // Never another owner's line, whatever the id says.
  if (line.owner !== owner) return null;
  const known = line.versions.find((v) => v.hash === hash);
  if (known) return { id, n: known.n, line, added: false };
  const n = (line.versions.length ? line.versions[line.versions.length - 1].n : 0) + 1;
  const previous = line.current;
  line.versions.push({ n, hash, version: String(version || ""), at: now, size: Number(size) || 0 });
  if (line.versions.length > MAX_VERSIONS) line.versions = line.versions.slice(-MAX_VERSIONS);
  line.current = n;
  line.name = name || line.name;
  line.updated = now;
  await env.APPS.put(`line:${id}`, JSON.stringify(line));
  return { id, n, line, added: true, previous };
}

/// Mark which of a line's builds is the gallery's row: the current one
/// listed, every other one superseded (kept, with its link, off the shelf).
export async function settleRows(env, line) {
  for (const v of line.versions) {
    let meta;
    try {
      meta = JSON.parse((await env.APPS.get(`app:${v.hash}`)) || "null");
    } catch (_) {
      meta = null;
    }
    if (!meta) continue;
    const superseded = v.n !== line.current;
    if (Boolean(meta.superseded) === superseded && meta.line === line.id && meta.version_n === v.n) continue;
    meta.superseded = superseded;
    meta.line = line.id;
    meta.version_n = v.n;
    await env.APPS.put(`app:${v.hash}`, JSON.stringify(meta));
  }
}

/// What a person sees of a line: its name, current version and link; the
/// owner also sees every version.
export function lineView(line, base, { owner = false } = {}) {
  const cur = line.versions.find((v) => v.n === line.current) || null;
  const view = {
    id: line.id,
    name: line.name,
    app_id: line.app_id,
    url: `${base}/a/${line.id}`,
    current: line.current,
    current_hash: cur ? cur.hash : null,
    versions: line.versions.length,
    updated: line.updated || line.created || null,
  };
  if (owner) {
    view.history = line.versions.map((v) => ({
      n: v.n,
      version: v.version,
      at: v.at,
      size: v.size,
      hash: v.hash,
      url: `${base}/a/${v.hash}`,
      current: v.n === line.current,
    }));
  }
  return view;
}

/// GET /lines/<id>, GET /lines (the caller's own), POST /lines/<id>/current.
export async function linesRoute(request, url, env, { ownerOf, identity, base }) {
  const rest = url.pathname.replace(/^\/lines\/?/, "").split("/").filter(Boolean);
  const me = identity ? ownerOf(identity) : null;
  // GET /lines?app=<manifest app id>: the caller's own app with that id, so
  // Studio can say "Publish update (v3)" before it publishes.
  if (rest.length === 0 && request.method === "GET") {
    if (!me) return { status: 401, body: { error: "sign in to see your apps" } };
    const appId = String(url.searchParams.get("app") || "").slice(0, 200);
    if (!appId) return { status: 400, body: { error: "say which app (?app=<its id>)" } };
    const line = await readLine(env, await lineIdOf(me, appId));
    if (!line || line.owner !== me) return { status: 404, body: { error: "not published yet" } };
    return { status: 200, body: lineView(line, base, { owner: true }) };
  }
  const id = rest[0];
  const line = await readLine(env, id);
  if (!line) return { status: 404, body: { error: "no such app" } };
  const owns = me && me === line.owner;
  if (rest.length === 1 && request.method === "GET") {
    return { status: 200, body: lineView(line, base, { owner: owns }) };
  }
  if (rest.length === 2 && rest[1] === "current" && request.method === "POST") {
    if (!owns) return { status: 403, body: { error: "only the app's publisher can choose its current version" } };
    let ask = {};
    try {
      ask = await request.json();
    } catch (_) {
      ask = {};
    }
    const n = Number(ask.version);
    const v = line.versions.find((x) => x.n === n);
    if (!v) return { status: 404, body: { error: `there is no version ${ask.version}` } };
    if (await env.APPS.get(`takedown:${v.hash}`)) return { status: 409, body: { error: "that version was taken down" } };
    if (line.current !== n) {
      line.current = n;
      line.updated = Math.floor(Date.now() / 1000);
      await env.APPS.put(`line:${line.id}`, JSON.stringify(line));
      await settleRows(env, line);
    }
    return { status: 200, body: lineView(line, base, { owner: true }) };
  }
  return { status: 404, body: { error: "not found" } };
}

/// A version was unpublished: it leaves its line, and when it was current
/// the newest version left becomes current, so the app's link keeps working.
/// A line with no versions left is removed.
export async function dropVersion(env, hash, row) {
  const at = row && row.line ? { line: row.line } : null;
  if (!at) return;
  const line = await readLine(env, at.line);
  if (!line) return;
  line.versions = line.versions.filter((v) => v.hash !== hash);
  if (!line.versions.length) {
    await env.APPS.delete(`line:${line.id}`);
    return;
  }
  if (!line.versions.some((v) => v.n === line.current)) line.current = line.versions[line.versions.length - 1].n;
  line.updated = Math.floor(Date.now() / 1000);
  await env.APPS.put(`line:${line.id}`, JSON.stringify(line));
  await settleRows(env, line);
}
