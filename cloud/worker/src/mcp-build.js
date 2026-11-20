// What Krate in Claude's write tools reach (IC-989 phase 2): the build
// service's guide and compile doors, each account's build limits, and the
// record of whose build an id is.
//
// The contract with the builder (cloud/builder/src/server.js):
//
//   POST /compile           x-krate-builder: <secret>, x-krate-account: hub:<owner>
//                           { name, files: [{ path, data: base64 }] }
//                           -> 202 { id } | 4xx/5xx { error }
//   GET  /build/<id>        same two headers
//                           -> { state: building|done|failed, stage, line,
//                                error, check: check-app's { stage, kind,
//                                detail, fix } on a failed check, result }
//   GET  /build/<id>/file   same two headers -> the UNSIGNED .krate; the hub
//                           signs it as the account (signAsAccount) when it
//                           publishes it
//   GET  /guide             x-krate-builder only -> text/markdown
//
// The builder keeps a job to the account it was started for, and counts its
// own per-account limits on that same `hub:<owner>`. The compile door runs
// check-app and pack only -- no model -- so nothing here can spend Krate's
// AI key. test/mcp-build-e2e.test.mjs runs this against the real builder.

/** A build an account may start: this many an hour and a day (env overrides). */
export function buildLimits(env) {
  return {
    hour: Number(env.MCP_BUILDS_PER_HOUR) > 0 ? Number(env.MCP_BUILDS_PER_HOUR) : 10,
    day: Number(env.MCP_BUILDS_PER_DAY) > 0 ? Number(env.MCP_BUILDS_PER_DAY) : 30,
  };
}

const builderUrl = (env) => String(env.KRATE_BUILDER_URL || "").replace(/\/$/, "");
const asHub = (env, owner) => ({
  "x-krate-builder": env.KRATE_BUILDER_SECRET || "",
  ...(owner ? { "x-krate-account": `hub:${owner}` } : {}),
});

/** The guide, kept an hour per isolate: it changes only with the builder's Krate. */
let guideCache = null;
export async function fetchGuide(env) {
  if (guideCache && guideCache.until > Date.now() && guideCache.from === builderUrl(env)) return guideCache.text;
  if (!builderUrl(env) || !env.KRATE_BUILDER_SECRET) return null;
  try {
    const res = await fetch(`${builderUrl(env)}/guide`, { headers: asHub(env) });
    if (!res.ok) return null;
    const text = await res.text();
    if (!text.trim()) return null;
    guideCache = { text, until: Date.now() + 60 * 60 * 1000, from: builderUrl(env) };
    return text;
  } catch (_) {
    return null;
  }
}

/** Tests only: forget the guide. */
export function __forgetGuide() {
  guideCache = null;
}

/**
 * Whether `owner` may start another build: { over } names the limit it
 * would pass, else null, with what to record once the build is accepted.
 * A build the builder refuses is never counted.
 */
export async function buildAllowance(env, owner) {
  const t = Math.floor(Date.now() / 1000);
  let held = [];
  try {
    held = JSON.parse((await env.APPS.get(`mcpbuilds:${owner}`)) || "[]").filter((x) => Number.isFinite(x) && x > t - 86400);
  } catch (_) {
    held = [];
  }
  const { hour, day } = buildLimits(env);
  if (held.filter((x) => x > t - 3600).length >= hour) return { over: `${hour} builds an hour`, held };
  if (held.length >= day) return { over: `${day} builds a day`, held };
  return { over: null, held };
}

/** Count an accepted build: one KV write. */
export async function noteBuild(env, owner, held) {
  await env.APPS.put(`mcpbuilds:${owner}`, JSON.stringify([...held, Math.floor(Date.now() / 1000)]), { expirationTtl: 2 * 86400 });
}

const recordKey = (id) => `mcpbuild:${id}`;
export async function readRecord(env, id) {
  try {
    return JSON.parse((await env.APPS.get(recordKey(id))) || "null");
  } catch (_) {
    return null;
  }
}
export async function writeRecord(env, id, record) {
  await env.APPS.put(recordKey(id), JSON.stringify(record), { expirationTtl: 30 * 86400 });
}

/** Send files to the builder's compile door: { id } | { status, words }. */
export async function compile(env, owner, name, files) {
  if (!builderUrl(env) || !env.KRATE_BUILDER_SECRET) return { status: 503, words: "Building is not set up on this hub." };
  let res;
  try {
    res = await fetch(`${builderUrl(env)}/compile`, {
      method: "POST",
      headers: { "content-type": "application/json", ...asHub(env, owner) },
      body: JSON.stringify({ name, files }),
    });
  } catch (_) {
    return { status: 502, words: "Krate's build service could not be reached. Try again in a minute." };
  }
  if (res.status === 202 || res.status === 200) {
    const body = await res.json().catch(() => ({}));
    if (/^[0-9a-f]{32}$/.test(body.id || "")) return { id: body.id };
    return { status: 502, words: "Krate's build service gave no build id." };
  }
  // The builder refuses in JSON { error } -- words written for the model
  // (a path it does not take, a crate it does not allow) -- and a plain
  // text body from anything in front of it.
  const said = await res.text().catch(() => "");
  let words = said;
  try {
    const parsed = JSON.parse(said);
    if (parsed && typeof parsed.error === "string") words = parsed.error;
  } catch (_) {
    /* plain text */
  }
  return { status: res.status, words: words.slice(0, 500) || `the build service answered ${res.status}` };
}

/** The builder's record of one job: its JSON, or { status } when it has none. */
export async function jobStatus(env, owner, id) {
  try {
    const res = await fetch(`${builderUrl(env)}/build/${id}`, { headers: asHub(env, owner) });
    if (!res.ok) return { status: res.status };
    return await res.json();
  } catch (_) {
    return { status: 502 };
  }
}

/** The finished file, or null. */
export async function jobFile(env, owner, id) {
  try {
    const res = await fetch(`${builderUrl(env)}/build/${id}/file`, { headers: asHub(env, owner) });
    if (!res.ok) return null;
    return new Uint8Array(await res.arrayBuffer());
  } catch (_) {
    return null;
  }
}
