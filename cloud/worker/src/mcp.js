// Krate as a remote MCP server (IC-989, phase 1): POST /mcp.
//
// What a Claude connector (or any MCP client) talks to. Streamable HTTP,
// answered with plain JSON: every request gets one JSON-RPC response in the
// POST's own answer, a notification gets 202 and no body, and the server
// never opens a stream of its own, so GET /mcp is 405. No sessions: every
// tool here reads public data, so nothing has to be remembered between
// requests, and an Mcp-Session-Id a client sends is accepted and ignored.
//
// Phase 1, no sign-in: three tools read what the public gallery shows --
// the same ranked search (searchRelevance, test 1315), the same "Start
// here" and newest shelves, the same takedown and unlisted rules --
// through the gallery code in index.js, handed in as `deps`, never a copy
// of it.
//
// Phase 2, CLAUDE WRITES, KRATE BUILDS: get_krate_guide (no sign-in) gives
// the chat model Krate's authoring guide; build_app sends the files it
// wrote to the build service, which compiles and checks them with NO model
// of ours; build_status reports check-app's own words on a failure, and on
// a pass publishes the app unlisted to the signed-in person's account and
// answers a link that opens it on any device. The two write tools need an
// OAuth sign-in (mcp-oauth.js); without one they answer 401 with the
// resource metadata, which is what starts a client's sign-in. No tool ever
// starts an AI build: that would spend Krate's key.
//
// JSON-RPC batches are refused: MCP 2025-06-18 removed them, and the
// streamable HTTP clients that matter never send one.

import { signInNeeded } from "./mcp-oauth.js";

/** The protocol versions this server speaks, newest first. */
export const MCP_VERSIONS = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

/** This MCP surface's own version, not the hub's or Krate's. */
export const MCP_SERVER_VERSION = "1.0.0";

/** A request body over this is refused before it is parsed: room for
 * build_app's 2 MB of source with JSON's escaping, and no more. */
const MAX_BODY = 4 * 1024 * 1024;

/** The most results a search answers with. */
const MAX_RESULTS = 20;

/** The gallery's shelves, as publish keeps them (shelfOf in index.js). */
export const CATEGORIES = ["apps", "games", "productivity", "tools", "media", "learning"];

/** Headers every /mcp answer carries: open to any origin, like the public GET endpoints. */
const MCP_CORS = {
  "access-control-allow-origin": "*",
  "access-control-allow-methods": "POST, GET, DELETE, OPTIONS",
  "access-control-allow-headers": "content-type, accept, authorization, mcp-protocol-version, mcp-session-id, last-event-id",
  "access-control-expose-headers": "mcp-session-id, retry-after, www-authenticate",
};

/** Add the /mcp CORS headers to any response, the rate limiter's 429 included. */
export function mcpCors(res) {
  const out = new Response(res.body, res);
  for (const [k, v] of Object.entries(MCP_CORS)) out.headers.set(k, v);
  return out;
}

/** The browser's preflight for /mcp. */
export function mcpPreflight() {
  return mcpCors(new Response(null, { status: 204 }));
}

const jsonResponse = (body, status = 200) =>
  mcpCors(new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } }));

const rpcError = (id, code, message, status = 200) =>
  jsonResponse({ jsonrpc: "2.0", id: id === undefined ? null : id, error: { code, message } }, status);

/** A tool's arguments that do not fit its schema, or an unknown tool. */
class BadParams extends Error {}
/** A tool that needs the person signed in, called without a sign-in. */
class SignInFirst extends Error {}

// ------------------------------------------------------------------ schemas

const PERMISSION = {
  type: "object",
  properties: {
    id: { type: "string", description: "The capability as Krate names it, e.g. store.sql" },
    words: { type: "string", description: "What it lets the app do, in plain words" },
    why: { type: "string", description: "The app's own reason, as its maker wrote it" },
    required: { type: "boolean" },
  },
  required: ["id", "words", "required"],
};

const APP = {
  type: "object",
  properties: {
    id: { type: "string", description: "The build's content hash (SHA-256)" },
    name: { type: "string" },
    description: { type: "string" },
    category: { type: "string" },
    publisher: { type: "string" },
    verified_publisher: { type: "boolean", description: "GitHub vouched for the publisher's account" },
    signed: { type: "boolean", description: "The app carries a publisher signature that verified" },
    link: { type: "string", description: "The app's own link; it always opens the current version" },
    browser_link: { type: "string", description: "Opens the app in a browser tab, nothing to install" },
    screenshot: { type: ["string", "null"] },
    permissions: { type: ["array", "null"], items: PERMISSION, description: "null when the app was published before permissions were recorded" },
    size_bytes: { type: "integer" },
  },
  required: ["id", "name", "description", "category", "publisher", "verified_publisher", "signed", "link", "browser_link", "permissions", "size_bytes"],
};

const APP_DETAIL = {
  type: "object",
  properties: {
    ...APP.properties,
    published: { type: ["string", "null"], description: "When this build was published (ISO 8601)" },
    versions: {
      type: ["object", "null"],
      properties: {
        count: { type: "integer" },
        current: { type: "integer", description: "The version the app's link opens now" },
        this_version: { type: "integer" },
        newer_exists: { type: "boolean" },
      },
    },
  },
  required: [...APP.required, "published", "versions"],
};

const READ_ONLY = { readOnlyHint: true, openWorldHint: false };
/** A tool that makes something new on the person's account and removes nothing. */
const WRITES = { readOnlyHint: false, destructiveHint: false, idempotentHint: false, openWorldHint: false };

/** Every write tool, which needs the person signed in. */
export const NEEDS_SIGN_IN = new Set(["build_app", "build_status"]);

const BUILD_ANSWER = {
  type: "object",
  properties: {
    build_id: { type: "string" },
    status: { type: "string", enum: ["building", "failed", "published", "refused", "unknown"] },
    message: { type: "string" },
    stage: { type: "string", description: "Where Krate's check stopped, on a failure" },
    detail: { type: "string", description: "What went wrong, in the check's own words" },
    fix: { type: "string", description: "How to fix it, in the check's own words" },
    name: { type: "string" },
    app_id: { type: "string" },
    link: { type: "string", description: "The app's own link; it opens the newest version" },
    browser_link: { type: "string", description: "Opens the app in a browser on any device, nothing to install" },
  },
  required: ["build_id", "status", "message"],
};

export const TOOLS = [
  {
    name: "search_apps",
    title: "Search Krate apps",
    description:
      "Search the public Krate gallery for apps by words. Results are ranked by how well the words match the app's name, " +
      "then its description and publisher. Each result has the app's link (opens in Krate), a link that opens it in a browser tab, " +
      "what it is allowed to do in plain words, its size, and whether its publisher is verified and the app signed.",
    inputSchema: {
      type: "object",
      properties: {
        query: { type: "string", minLength: 1, maxLength: 64, description: "Words to look for, e.g. \"markdown editor\"" },
        category: { type: "string", enum: CATEGORIES, description: "Only apps on this shelf" },
        limit: { type: "integer", minimum: 1, maximum: MAX_RESULTS, description: `How many results, at most ${MAX_RESULTS} (default 10)` },
      },
      required: ["query"],
      additionalProperties: false,
    },
    outputSchema: {
      type: "object",
      properties: {
        query: { type: "string" },
        category: { type: ["string", "null"] },
        results: { type: "array", items: APP },
      },
      required: ["query", "category", "results"],
    },
    annotations: READ_ONLY,
  },
  {
    name: "get_app",
    title: "Get a Krate app",
    description:
      "One public Krate app's details, from its link or id: what it does, what it is allowed to do in plain words, " +
      "its versions, size, publisher and signature, its link and its browser link. Accepts a hub link " +
      "(https://hub.krate.tech/a/..., /run/...), the app's own id (v followed by 11 hex characters), a short id, or a full hash. " +
      "An app removed from the registry answers with its removal notice only.",
    inputSchema: {
      type: "object",
      properties: {
        id_or_link: { type: "string", minLength: 1, maxLength: 512, description: "A Krate app link, or its id" },
      },
      required: ["id_or_link"],
      additionalProperties: false,
    },
    outputSchema: {
      type: "object",
      properties: {
        found: { type: "boolean" },
        removed: { type: "boolean", description: "The app was taken down from the registry" },
        notice: { type: "string", description: "The removal notice, when removed" },
        message: { type: "string" },
        app: APP_DETAIL,
      },
      required: ["found", "removed"],
    },
    annotations: READ_ONLY,
  },
  {
    name: "list_featured",
    title: "Featured Krate apps",
    description:
      "The Krate gallery's front shelves: the apps picked to start with, and the newest apps. A good first look when " +
      "someone asks what Krate apps exist.",
    inputSchema: { type: "object", properties: {}, additionalProperties: false },
    outputSchema: {
      type: "object",
      properties: {
        start_here: { type: "array", items: APP },
        newest: { type: "array", items: APP },
        newest_label: { type: "string" },
      },
      required: ["start_here", "newest", "newest_label"],
    },
    annotations: READ_ONLY,
  },
  {
    name: "get_krate_guide",
    title: "Read the Krate app guide",
    description:
      "Krate's guide to writing a Krate app: the files an app is made of (Cargo.toml, src/lib.rs, manifest.toml), " +
      "the API it may call, the permissions it may declare, and worked examples. Read it before writing any Krate app. " +
      "Then send the files to build_app instead of pasting them into the chat. No sign-in needed.",
    inputSchema: { type: "object", properties: {}, additionalProperties: false },
    outputSchema: {
      type: "object",
      properties: {
        characters: { type: "integer", description: "The guide's length; the guide itself is the text content" },
        next: { type: "string" },
      },
      required: ["characters", "next"],
    },
    annotations: READ_ONLY,
  },
  {
    name: "build_app",
    title: "Build and publish a Krate app",
    description:
      "Compile the Krate app you wrote and check it with Krate's own check: it builds the code, confirms it uses only " +
      "Krate's API, and runs it once. Send every file each time (Cargo.toml, src/lib.rs, manifest.toml and any assets), " +
      "written as get_krate_guide says. Answers a build id at once; then call build_status until it is done. When the app " +
      "passes, Krate publishes it unlisted to the signed-in person's account, with a link that opens it on any device. " +
      "Needs the person to sign in to Krate.",
    inputSchema: {
      type: "object",
      properties: {
        name: { type: "string", minLength: 1, maxLength: 60, description: "The app's name, as a person would say it" },
        files: {
          type: "array",
          minItems: 1,
          maxItems: 64,
          description: "Every file of the app, with paths relative to the app folder",
          items: {
            type: "object",
            properties: {
              path: { type: "string", minLength: 1, maxLength: 200, description: "e.g. src/lib.rs" },
              content: { type: "string", description: "The file's text, or base64 when encoding is base64" },
              encoding: { type: "string", enum: ["utf8", "base64"], description: "base64 for a picture or other binary asset" },
            },
            required: ["path", "content"],
            additionalProperties: false,
          },
        },
      },
      required: ["name", "files"],
      additionalProperties: false,
    },
    outputSchema: BUILD_ANSWER,
    annotations: WRITES,
  },
  {
    name: "build_status",
    title: "Check a Krate build",
    description:
      "Where a build_app build is. While it runs, call again in about 30 seconds. If it failed, the answer has Krate's " +
      "check in its own words -- the stage, what went wrong, how to fix it: fix the code and call build_app again with " +
      "every file. When it passes, the app is published unlisted to the person's Krate account and the answer has its " +
      "link and a browser link that opens it on any device. Needs the person to sign in to Krate.",
    inputSchema: {
      type: "object",
      properties: {
        build_id: { type: "string", minLength: 32, maxLength: 32, description: "The id build_app answered" },
      },
      required: ["build_id"],
      additionalProperties: false,
    },
    outputSchema: BUILD_ANSWER,
    annotations: WRITES,
  },
];

// The directory reads a tool's name for its listing from annotations.title
// (MCP 2025-06-18 and later), and flags every tool without one; an older
// client reads the top-level title. One source: the top-level title, copied.
for (const tool of TOOLS) tool.annotations = { title: tool.title, ...tool.annotations };

// --------------------------------------------------------------- arguments

/** Check `args` against a tool's input schema (the subset these tools use). */
function checkArgs(tool, args) {
  const schema = tool.inputSchema;
  if (args === undefined || args === null) args = {};
  if (typeof args !== "object" || Array.isArray(args)) throw new BadParams("arguments must be an object");
  for (const key of Object.keys(args)) {
    if (!(key in schema.properties)) throw new BadParams(`${tool.name} takes no argument "${key}"`);
  }
  for (const key of schema.required || []) {
    if (args[key] === undefined) throw new BadParams(`${tool.name} needs "${key}"`);
  }
  for (const [key, rule] of Object.entries(schema.properties)) {
    const v = args[key];
    if (v === undefined) continue;
    if (rule.type === "string") {
      if (typeof v !== "string") throw new BadParams(`"${key}" must be a string`);
      if (rule.minLength && v.trim().length < rule.minLength) throw new BadParams(`"${key}" must not be empty`);
      if (rule.maxLength && v.length > rule.maxLength) throw new BadParams(`"${key}" is at most ${rule.maxLength} characters`);
      if (rule.enum && !rule.enum.includes(v)) throw new BadParams(`"${key}" must be one of ${rule.enum.join(", ")}`);
    } else if (rule.type === "integer") {
      if (!Number.isInteger(v)) throw new BadParams(`"${key}" must be a whole number`);
      if (v < rule.minimum || v > rule.maximum) throw new BadParams(`"${key}" must be ${rule.minimum} to ${rule.maximum}`);
    } else if (rule.type === "array") {
      if (!Array.isArray(v)) throw new BadParams(`"${key}" must be a list`);
      if (v.length < (rule.minItems || 0) || v.length > (rule.maxItems || Infinity)) {
        throw new BadParams(`"${key}" holds ${rule.minItems} to ${rule.maxItems} items`);
      }
    }
  }
  return args;
}

// ------------------------------------------------------------------ shaping

/** Bytes as a person reads them. */
function sizeWords(n) {
  if (!Number.isFinite(n) || n <= 0) return "size not recorded";
  if (n < 1024) return `${n} bytes`;
  if (n < 1024 * 1024) return `${Math.round(n / 1024)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}

/** A listing's publish time, seconds or milliseconds, as ISO 8601. */
function isoDate(published) {
  const n = Number(published);
  if (!Number.isFinite(n) || n <= 0) return null;
  return new Date(n > 1e12 ? n : n * 1000).toISOString();
}

/**
 * A listing's permissions in plain words: the validator's own words, kept
 * with the listing since IC-989; an app published before that shows its
 * capability id and its maker's reason. Null when none were recorded --
 * an absent list and an empty one mean very different things.
 */
function permissionsOf(meta) {
  if (!Array.isArray(meta.capabilities)) return null;
  return meta.capabilities.slice(0, 64).map((c) => ({
    id: String(c.cap || ""),
    words: String(c.words || c.cap || ""),
    ...(c.rationale ? { why: String(c.rationale) } : {}),
    required: c.required !== false,
  }));
}

/** One app as every tool shows it, from a gallery row or a listing. */
function appOf(deps, { id, meta, url, shot, identity_checks }) {
  const ref = meta.line || id;
  return {
    id,
    name: String(meta.name || "Untitled app"),
    description: String(meta.description || ""),
    category: String(meta.category || "apps"),
    publisher: String(meta.author || ""),
    verified_publisher: identity_checks ? Boolean(identity_checks.verified_publisher) : Boolean(meta.author_url),
    signed: identity_checks
      ? Boolean(identity_checks.signed)
      : Boolean(meta.signature && meta.signature.state === "valid"),
    link: url || `${deps.base}/a/${ref}`,
    browser_link: `${deps.base}/run/${ref}`,
    screenshot: shot || null,
    permissions: permissionsOf(meta),
    size_bytes: Number.isFinite(Number(meta.size)) ? Number(meta.size) : 0,
  };
}

/** A few readable lines for one app. */
function appText(app, n) {
  const head = `${n ? `${n}. ` : ""}${app.name}${app.description ? ` -- ${app.description}` : ""}`;
  const who = app.publisher
    ? `by ${app.publisher}${app.verified_publisher ? " (verified publisher)" : ""}`
    : "publisher not named";
  const perms = app.permissions === null
    ? "permissions not recorded"
    : app.permissions.length
      ? `may: ${app.permissions.map((p) => p.words).join("; ")}`
      : "asks for no permissions";
  return [
    head,
    `   ${app.category}, ${sizeWords(app.size_bytes)}, ${who}, ${app.signed ? "signed" : "unsigned"}`,
    `   ${perms}`,
    `   open: ${app.link}  in a browser: ${app.browser_link}`,
  ].join("\n");
}

// -------------------------------------------------------------------- tools

async function searchApps(args, deps) {
  const query = args.query.trim().slice(0, 64);
  const limit = args.limit || 10;
  const params = new URLSearchParams({ q: query });
  if (args.category) params.set("cat", args.category);
  const payload = await deps.gallery(params);
  if (!payload) return failure("The Krate gallery could not be read just now. Try again in a minute.");
  const results = (payload.results || []).slice(0, limit).map((r) => appOf(deps, r));
  const structured = { query, category: args.category || null, results };
  const textOut = results.length
    ? `${results.length} Krate app${results.length === 1 ? "" : "s"} for "${query}"${args.category ? ` in ${args.category}` : ""}, best match first:\n\n` +
      results.map((a, i) => appText(a, i + 1)).join("\n\n")
    : `No public Krate app matches "${query}"${args.category ? ` in ${args.category}` : ""}. Every word has to appear in an app's name, description or publisher.`;
  return { content: [{ type: "text", text: textOut }], structuredContent: structured };
}

/** The ref inside a link or id a person pasted, or null. */
export function refOf(input) {
  let s = String(input || "").trim();
  if (/^https?:\/\//i.test(s)) {
    let url;
    try {
      url = new URL(s);
    } catch (_) {
      return null;
    }
    const path = /^\/(?:a|run|meta)\/([^/?#]+)/.exec(url.pathname);
    s = path ? path[1] : url.searchParams.get("a") || "";
  } else {
    s = s.replace(/^\/?(?:a|run|meta)\//, "").replace(/[/?#].*$/, "");
  }
  s = s.toLowerCase();
  return /^v[0-9a-f]{11}$/.test(s) || /^[0-9a-f]{8,64}$/.test(s) ? s : null;
}

async function getApp(args, deps) {
  const ref = refOf(args.id_or_link);
  if (!ref) {
    return failure(
      "That is not a Krate app link or id. A link looks like https://hub.krate.tech/a/v1a2b3c4d5e6, an id like v1a2b3c4d5e6.",
      { found: false, removed: false },
    );
  }
  const found = await deps.findApp(ref);
  if (found && found.takedown) {
    const notice = `${deps.base}/takedown/${found.hash}`;
    return {
      content: [{ type: "text", text: `This app was removed from the Krate registry. The notice: ${notice}` }],
      structuredContent: { found: false, removed: true, notice },
    };
  }
  // An unlisted app has a working link and no public face here: this
  // server answers for the public gallery only.
  if (!found || !found.row || found.row.unlisted) {
    return failure("No public Krate app has that link or id.", { found: false, removed: false });
  }
  const { hash, row } = found;
  const v = await deps.versionInfo(hash);
  const app = {
    ...appOf(deps, { id: hash, meta: row, shot: found.shot || null }),
    published: isoDate(row.published),
    versions: v ? { count: v.versions, current: v.current, this_version: v.n, newer_exists: v.newer } : null,
  };
  const lines = [appText(app)];
  if (app.published) lines.push(`   published ${app.published.slice(0, 10)}`);
  if (app.versions) {
    lines.push(
      `   version ${app.versions.this_version} of ${app.versions.count}` +
        (app.versions.newer_exists ? `; the app's link opens version ${app.versions.current}, which is newer` : ""),
    );
  }
  return { content: [{ type: "text", text: lines.join("\n") }], structuredContent: { found: true, removed: false, app } };
}

async function listFeatured(_args, deps) {
  const payload = await deps.gallery(new URLSearchParams());
  if (!payload) return failure("The Krate gallery could not be read just now. Try again in a minute.");
  const shelf = (id) => (payload.shelves || []).find((s) => s.id === id);
  const SMALL = 6;
  const startHere = ((shelf("picks") || {}).apps || []).slice(0, SMALL).map((r) => appOf(deps, r));
  const newShelf = shelf("new") || { label: "Newest", apps: [] };
  const newest = (newShelf.apps || []).slice(0, SMALL).map((r) => appOf(deps, r));
  const parts = [];
  if (startHere.length) parts.push(`Start here:\n\n${startHere.map((a, i) => appText(a, i + 1)).join("\n\n")}`);
  if (newest.length) parts.push(`${newShelf.label}:\n\n${newest.map((a, i) => appText(a, i + 1)).join("\n\n")}`);
  return {
    content: [{ type: "text", text: parts.length ? parts.join("\n\n") : "The Krate gallery has no public apps yet." }],
    structuredContent: { start_here: startHere, newest, newest_label: newShelf.label || "Newest" },
  };
}

/** A tool that ran and could not do what was asked: said in words, isError. */
function failure(words, structured) {
  return {
    isError: true,
    content: [{ type: "text", text: words }],
    ...(structured ? { structuredContent: { ...structured, message: words } } : {}),
  };
}

// ------------------------------------------------------- phase 2: building

/** The most source one build may carry, in characters (base64 counts as sent). */
const MAX_SOURCE_CHARS = 2 * 1024 * 1024;

async function getGuide(_args, deps) {
  const guide = await deps.guide();
  if (!guide) return failure("Krate's guide could not be fetched just now. Try again in a minute.", { characters: 0, next: "get_krate_guide" });
  const lead =
    "Write the app's files as this guide says, then call build_app with all of them. Do not paste the files into the chat " +
    "unless the person asks: Krate checks them, and build_status gives you its words if anything needs fixing.\n\n" +
    "Start src/lib.rs with `use krate::prelude::*;`. A Krate app is no_std, where x.floor(), round, sqrt, sin and atan2 " +
    "do not exist on f32 or f64 without it; the first app built through this connector failed its first build on exactly that.\n\n";
  return { content: [{ type: "text", text: lead + guide }], structuredContent: { characters: guide.length, next: "build_app" } };
}

/** The float methods `krate::prelude::*` gives `f32` and `f64` in a no_std app. */
const PRELUDE_FLOAT_METHODS = new Set([
  "floor", "ceil", "round", "trunc", "abs", "sqrt", "cbrt", "exp", "exp2", "ln", "log2", "log10",
  "sin", "cos", "tan", "asin", "acos", "atan", "sinh", "cosh", "tanh", "powf", "atan2", "hypot",
  "copysign", "fract", "powi", "sin_cos", "rem_euclid", "mul_add",
]);

/**
 * The compiler's words without cargo's progress lines. A failed build's
 * detail opened with a dozen "Downloaded" / "Compiling" lines, and claude.ai
 * cut the answer ("Too long to show in full") before rustc's own help.
 */
export function compilerWords(detail) {
  const noise = /^\s*(Compiling|Downloading|Downloaded|Updating|Locking|Generating bindings|Fetching|Blocking)\b/;
  const kept = detail.split("\n").filter((line) => !noise.test(line));
  return kept.join("\n").replace(/\n{3,}/g, "\n\n").trim();
}

/**
 * The check's fix as it reads to someone in a chat, with no Krate on their
 * machine. The builder runs a released Krate, whose advice can lag what the
 * hub knows (crates/cli build_fix has the same float case from the next
 * release on), and whose generic advice -- "run `krate doctor` if the target
 * or toolchain looks wrong" -- means nothing here: the builder's toolchain is
 * Krate's own.
 */
export function remoteFix(detail, fix) {
  const m = /no method named `([a-z0-9_]+)` found for type `(f32|f64|\{float\})`/.exec(detail);
  if (m && PRELUDE_FLOAT_METHODS.has(m[1])) {
    return (
      `\`${m[1]}\` is a std float method, and a Krate app is no_std. Do not write your own: add ` +
      "`use krate::prelude::*;` at the top of src/lib.rs (and of any other file that calls it). It gives f32 and f64 " +
      "the floor, round, sqrt, sin, atan2 and the rest of std's maths, with the same meanings. Then call build_app again."
    );
  }
  if (/krate doctor|rustup's toolchain/.test(fix)) {
    return "Fix the compiler errors above in the app's code. The builder's own toolchain is Krate's and needs nothing from you.";
  }
  return fix;
}

/** The files as the builder takes them: relative paths, base64 data. */
function sourceFiles(files) {
  const seen = new Set();
  let total = 0;
  const out = [];
  for (const f of files) {
    if (!f || typeof f !== "object" || Array.isArray(f)) throw new BadParams("each file is { path, content }");
    for (const key of Object.keys(f)) if (!["path", "content", "encoding"].includes(key)) throw new BadParams(`a file takes no "${key}"`);
    if (typeof f.path !== "string" || typeof f.content !== "string") throw new BadParams("each file has a path and its content as text");
    const path = f.path.trim().replace(/^\.\//, "");
    if (!path || path.length > 200 || path.startsWith("/") || path.includes("\\") || path.split("/").some((p) => p === "" || p === "." || p === "..")) {
      throw new BadParams(`"${f.path}" is not a path inside the app folder`);
    }
    if (seen.has(path)) throw new BadParams(`"${path}" is sent twice`);
    seen.add(path);
    if (f.encoding !== undefined && f.encoding !== "utf8" && f.encoding !== "base64") throw new BadParams(`"${path}": encoding is utf8 or base64`);
    total += f.content.length;
    if (total > MAX_SOURCE_CHARS) throw new BadParams("the files are more than 2 MB together; send the app's source, not its build output");
    let data;
    if (f.encoding === "base64") {
      if (!/^[A-Za-z0-9+/]*={0,2}$/.test(f.content.replace(/\s+/g, ""))) throw new BadParams(`"${path}" is not base64`);
      data = f.content.replace(/\s+/g, "");
    } else {
      const bytes = new TextEncoder().encode(f.content);
      let s = "";
      for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
      data = btoa(s);
    }
    out.push({ path, data });
  }
  for (const needed of ["Cargo.toml", "src/lib.rs", "manifest.toml"]) {
    if (!seen.has(needed)) throw new BadParams(`a Krate app needs ${needed}; send every file each time`);
  }
  return out;
}

function signedIn(deps) {
  if (!deps.who) throw new SignInFirst("sign in");
  return deps.who;
}

const answer2 = (structured, words, isError = false) => ({
  ...(isError ? { isError: true } : {}),
  content: [{ type: "text", text: words }],
  structuredContent: { ...structured, message: words },
});

async function buildApp(args, deps) {
  const who = signedIn(deps);
  const name = args.name.trim();
  const files = sourceFiles(args.files);
  const allowance = await deps.buildAllowance(who.owner);
  if (allowance.over) {
    return answer2({ build_id: "", status: "refused" }, `This Krate account has started ${allowance.over}, the most it may. Try again later.`, true);
  }
  const started = await deps.compile(who.owner, name, files);
  if (!started.id) {
    const words = started.status === 429
      ? "A build for this account is still running. Call build_status for it, and build again when it is done."
      : `Krate could not start the build: ${started.words}`;
    return answer2({ build_id: "", status: "refused" }, words, true);
  }
  await deps.noteBuild(who.owner, allowance.held);
  await deps.writeRecord(started.id, { owner: who.owner, name, at: Math.floor(Date.now() / 1000) });
  return answer2(
    { build_id: started.id, status: "building", name },
    `Building "${name}" (build ${started.id}). Krate is compiling and checking it, which usually takes one to five minutes. ` +
      `Call build_status with build_id "${started.id}" in about 30 seconds.`,
  );
}

function publishedAnswer(id, rec) {
  const p = rec.published;
  return answer2(
    { build_id: id, status: "published", name: rec.name, app_id: p.app_id, link: p.link, browser_link: p.browser_link },
    `"${rec.name}" is built, checked and published to your Krate account, unlisted: only people with the link can open it.\n\n` +
      `Open it in a browser on any device, nothing to install: ${p.browser_link}\n` +
      `Its own link, which also opens it in Krate on a computer and always opens the newest version: ${p.link}`,
  );
}

async function buildStatus(args, deps) {
  const who = signedIn(deps);
  const id = args.build_id.toLowerCase();
  if (!/^[0-9a-f]{32}$/.test(id)) throw new BadParams("build_id is the 32-character id build_app answered");
  const rec = await deps.readRecord(id);
  // Another account's build is answered exactly like one that never was.
  if (!rec || rec.owner !== who.owner) {
    return answer2({ build_id: id, status: "unknown" }, "No build of this Krate account has that id. Start one with build_app.", true);
  }
  if (rec.published) return publishedAnswer(id, rec);
  const job = await deps.jobStatus(who.owner, id);
  if (job.status) {
    return answer2(
      { build_id: id, status: "unknown" },
      job.status === 404
        ? "Krate's build service no longer has this build. Call build_app again with every file."
        : "Krate's build service could not be reached. Call build_status again in a minute.",
      true,
    );
  }
  // "building" is the builder's word for a compile in progress; "working"
  // is a build's, kept so an older builder still reads as in progress.
  if (job.state === "building" || job.state === "working") {
    return answer2(
      { build_id: id, status: "building", name: rec.name, stage: String(job.stage || "") },
      `Still building "${rec.name}"${job.line ? `: ${job.line}` : ""}. Call build_status again in about 30 seconds.`,
    );
  }
  if (job.state === "failed") {
    const check = job.check || {};
    const stage = String(check.stage || "check");
    const detail = compilerWords(String(check.detail || job.error || "the check did not pass"));
    const fix = remoteFix(detail, String(check.fix || ""));
    return answer2(
      { build_id: id, status: "failed", name: rec.name, stage, detail, fix },
      `"${rec.name}" did not pass Krate's check, at the ${stage} stage.\n\n${detail}\n\n` +
        (fix ? `How to fix it:\n${fix}\n\n` : "") +
        "Fix the code and call build_app again with every file.",
    );
  }
  if (job.state !== "done") {
    return answer2({ build_id: id, status: "unknown" }, `This build ${job.state || "ended"} before it finished. Call build_app again.`, true);
  }
  const bytes = await deps.jobFile(who.owner, id);
  if (!bytes) {
    return answer2({ build_id: id, status: "unknown" }, "The build passed, but its file could not be fetched. Call build_status again in a minute.", true);
  }
  const published = await deps.publishBuilt(who, bytes, rec.name);
  if (published.problem) {
    return answer2({ build_id: id, status: "failed", name: rec.name, stage: "publish", detail: published.problem, fix: "" },
      `"${rec.name}" passed the check, but Krate could not publish it: ${published.problem}`, true);
  }
  rec.published = {
    app_id: published.id,
    link: published.url,
    browser_link: `${deps.base}/run/${published.line || published.id}`,
  };
  await deps.writeRecord(id, rec);
  return publishedAnswer(id, rec);
}

const HANDLERS = {
  search_apps: searchApps,
  get_app: getApp,
  list_featured: listFeatured,
  get_krate_guide: getGuide,
  build_app: buildApp,
  build_status: buildStatus,
};

// ----------------------------------------------------------------- protocol

const INSTRUCTIONS =
  "Krate apps are small desktop apps anyone can make and share as one link. These tools read the public Krate gallery: " +
  "search it, look at one app, or see the featured apps. Every app lists what it may do in plain words before it runs. " +
  "Give people the app's link (opens in Krate) or its browser link (opens in a tab, nothing to install). " +
  "To make a new app: read get_krate_guide, write the files, send them to build_app, and call build_status until it " +
  "answers a link (or Krate's check's words, to fix and build again). Building needs the person to sign in to Krate; " +
  "Krate compiles and checks what you wrote and runs no AI of its own.";

function initialize(params) {
  const asked = params && typeof params.protocolVersion === "string" ? params.protocolVersion : null;
  return {
    protocolVersion: asked && MCP_VERSIONS.includes(asked) ? asked : MCP_VERSIONS[0],
    capabilities: { tools: { listChanged: false } },
    serverInfo: { name: "krate", title: "Krate", version: MCP_SERVER_VERSION, websiteUrl: "https://krate.tech" },
    instructions: INSTRUCTIONS,
  };
}

async function callTool(params, deps) {
  if (!params || typeof params !== "object") throw new BadParams("tools/call needs a tool name");
  const tool = TOOLS.find((t) => t.name === params.name);
  if (!tool) throw new BadParams(`Unknown tool: ${String(params.name).slice(0, 80)}`);
  // Asked before the arguments: a client learns to sign in first.
  if (NEEDS_SIGN_IN.has(tool.name) && !deps.who) throw new SignInFirst(tool.name);
  const args = checkArgs(tool, params.arguments);
  return await HANDLERS[tool.name](args, deps);
}

/** One JSON-RPC message's answer: a result, or { error }. */
async function answer(message, deps) {
  const { method, params } = message;
  switch (method) {
    case "initialize":
      return { result: initialize(params) };
    case "ping":
      return { result: {} };
    case "tools/list":
      return { result: { tools: TOOLS } };
    case "tools/call":
      try {
        return { result: await callTool(params, deps) };
      } catch (err) {
        if (err instanceof BadParams) return { error: { code: -32602, message: err.message } };
        if (err instanceof SignInFirst) return { signIn: true };
        console.error(`mcp tool failed: ${err && err.stack ? err.stack : err}`);
        return {
          result: failure("Something went wrong on Krate's side while answering. Try again in a minute."),
        };
      }
    default:
      return { error: { code: -32601, message: `Method not found: ${String(method).slice(0, 80)}` } };
  }
}

/**
 * The /mcp route. `deps` reaches the gallery code in index.js:
 *   base                 the hub's public address
 *   gallery(params)      the /apps payload for ?q= / ?cat= (or the shelves), or null
 *   findApp(ref)         { hash, row, shot } | { hash, takedown } | null
 *   versionInfo(hash)    { n, current, newer, versions } | null
 */
export async function mcpRoute(request, deps) {
  if (request.method === "OPTIONS") return mcpPreflight();
  if (request.method !== "POST") {
    // No server-initiated stream (GET) and no session to end (DELETE).
    return mcpCors(new Response("This MCP server answers POST only; it opens no event stream.", {
      status: 405,
      headers: { allow: "POST, OPTIONS", "content-type": "text/plain; charset=utf-8" },
    }));
  }
  const version = request.headers.get("mcp-protocol-version");
  if (version && !MCP_VERSIONS.includes(version)) {
    return rpcError(null, -32600, `Unsupported MCP-Protocol-Version ${version.slice(0, 32)}; this server speaks ${MCP_VERSIONS.join(", ")}`, 400);
  }
  const declared = Number(request.headers.get("content-length") || 0);
  if (declared > MAX_BODY) return rpcError(null, -32600, "Request too large", 413);
  const raw = await request.text();
  if (raw.length > MAX_BODY) return rpcError(null, -32600, "Request too large", 413);
  let message;
  try {
    message = JSON.parse(raw);
  } catch (_) {
    return rpcError(null, -32700, "Parse error: the body is not JSON", 400);
  }
  if (Array.isArray(message)) {
    return rpcError(null, -32600, "Batches are not supported (MCP 2025-06-18 removed them); send one message per request", 400);
  }
  if (!message || typeof message !== "object" || message.jsonrpc !== "2.0") {
    return rpcError(message && message.id, -32600, "Invalid Request: not a JSON-RPC 2.0 message", 400);
  }
  // Who is signed in: a token that is not a live access token for this
  // server is refused outright, as the spec asks; none means read-only.
  const auth = deps.authenticate ? await deps.authenticate(request) : null;
  if (auth && auth.invalid) return mcpCors(signInNeeded(deps.base, { invalid: true }));
  deps = { ...deps, who: auth ? auth.who : null };
  const accepted = () => mcpCors(new Response(null, { status: 202 }));
  if (typeof message.method !== "string") {
    // A response to something the server asked -- it asks nothing.
    if ("result" in message || "error" in message) return accepted();
    return rpcError(message.id, -32600, "Invalid Request: no method", 400);
  }
  // A notification (no id): accepted, nothing to say.
  if (!("id" in message)) return accepted();
  if (typeof message.id !== "string" && typeof message.id !== "number") {
    return rpcError(null, -32600, "Invalid Request: id must be a string or a number", 400);
  }
  const out = await answer(message, deps);
  // A write tool with no sign-in: the 401 whose header starts the client's
  // OAuth sign-in (MCP authorization).
  if (out.signIn) return mcpCors(signInNeeded(deps.base));
  return jsonResponse({ jsonrpc: "2.0", id: message.id, ...out });
}
