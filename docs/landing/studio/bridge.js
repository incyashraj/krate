/* Studio, in a browser tab.
 *
 * Studio's interface talks to its shell through exactly one function:
 *
 *   const invoke = (cmd, args) => tauri ? tauri.core.invoke(cmd, args) : ...
 *
 * Forty commands, one door. So the browser version is not a second UI --
 * it is a second implementation of that door. Studio's own HTML, CSS and
 * JavaScript are served unchanged, and this file answers what they ask.
 *
 * Why that matters beyond the time saved: the two surfaces cannot drift.
 * A fix to the build card or the send sheet lands on both at once, because
 * there is one copy of each.
 *
 * Where a browser genuinely cannot do what a desktop does, the answer is a
 * refusal in plain words -- never a silent pretence. Studio's UI already
 * handles a command failing; what it must never do is show someone a
 * button that quietly does nothing.
 */

const HUB = "https://hub.krate.tech";
const BUILDER = window.KRATE_BUILDER || "https://build.krate.tech";

const bridge = {
  token: null,
  me: null,
  job: null,          // the build in flight
  jobResult: null,    // its finished app
  poll: null,
};

/* The site's own session key. NOT a new one: /login/done already stores
 * the token here, and a second key would mean signing in twice for one
 * account. */
const TOKEN_KEY = "krate_tok";
/* What they were typing when the wall stopped them. */
const PENDING_KEY = "krate_pending_request";

try { bridge.token = localStorage.getItem(TOKEN_KEY); } catch (e) {}

/* A browser has no traffic lights, so the drawer's two-row header is one
 * row of wasted space.
 *
 * Studio splits its sidebar header in two because macOS draws the window
 * buttons over the first strip at x:20..74 -- the toggle sits on that line
 * and the mark takes the row beneath, clear of anything the system paints.
 * In a tab nothing is painted there, so the mark sat in a pocket of empty
 * drawer below the toggle instead of beside it. This collapses the lights
 * row and lifts the mark onto the toggle's line, which is what the desktop
 * would do too if the system were not in the way.
 *
 * Web-only, injected here rather than added to Studio's stylesheet: on a
 * desktop the two rows are correct and must stay. */
const WEB_CSS = `
body.macos .side-top, body:not(.macos) .side-top {
  height: 0; margin: 0; padding: 0; overflow: visible;
}
body.macos .side-brandrow, body:not(.macos) .side-brandrow {
  height: 44px; margin-top: 0; margin-bottom: 4px;
  padding: 0 2px; justify-content: space-between; align-items: center;
}
body.macos .side-top .side-toggle, body:not(.macos) .side-top .side-toggle {
  position: absolute; top: 7px; right: 12px; z-index: 3;
}
/* No traffic lights in a browser, so no 84px reservation for them.
   Studio adds that padding on macOS and a Mac browser gets body.macos,
   which pushed the back arrow and the session title a third of the way
   across the screen. The nudge that goes with it (top: -6px, to sit the
   row on the lights' line) goes too. */
body.macos .titlebar { padding-left: 12px; }
body.macos .titlebar > * { top: 0; }

/* On a phone the drawer is an off-canvas overlay, so anything positioned
   against it travels off-screen with it. The drawer's own toggle sits in
   the flow instead, and each VIEW has its own toggle for opening it. */
@media (max-width: 760px) {
  body.macos .side-top .side-toggle, body:not(.macos) .side-top .side-toggle {
    position: static;
  }
  body.macos .side-top, body:not(.macos) .side-top {
    height: 44px; display: flex; align-items: center; justify-content: flex-end;
    padding-right: 8px;
  }
}
`;
try {
  const sheet = document.createElement("style");
  sheet.textContent = WEB_CSS;
  document.head.appendChild(sheet);
} catch (e) {}

/* A tab has no onboarding. Every question it asks is already answered here:
 * the agent is ours and the only one, there is nothing to install, and the
 * name comes from the account they signed in with. Studio reads this flag
 * during boot, so it is written now -- before its script runs -- and the
 * onboarding view is never shown rather than shown and dismissed. */
try { localStorage.setItem("krate-onboarded", "1"); } catch (e) {}

async function hub(path, opts = {}) {
  const headers = { ...(opts.headers || {}) };
  if (bridge.token) headers.authorization = `Bearer ${bridge.token}`;
  if (opts.body && !headers["content-type"]) headers["content-type"] = "application/json";
  const res = await fetch(HUB + path, { ...opts, headers });
  if (!res.ok) throw new Error((await res.text().catch(() => "")) || res.statusText);
  return (res.headers.get("content-type") || "").includes("json") ? res.json() : res.text();
}

async function builder(path, opts = {}) {
  const headers = { ...(opts.headers || {}) };
  if (bridge.token) headers.authorization = `Bearer ${bridge.token}`;
  if (opts.body && !headers["content-type"]) headers["content-type"] = "application/json";
  const res = await fetch(BUILDER + path, { ...opts, headers });
  if (!res.ok) throw new Error((await res.text().catch(() => "")) || res.statusText);
  return (res.headers.get("content-type") || "").includes("json") ? res.json() : res.text();
}

/* The browser's half of the second key.
 *
 * A browser has no hardware id, and must not be given a fingerprint: a
 * canvas or font probe would identify people across the whole web, which
 * is a far bigger thing than counting a funded first app and not a trade
 * we are willing to make.
 *
 * So this is a plain random id, stored once. A determined person can clear
 * it, and that is accepted. What it stops is the ordinary case -- a second
 * email address for another funded app -- because the id survives signing
 * out and signing in as someone else. The account key catches the rest.
 */
function deviceId() {
  try {
    let id = localStorage.getItem("krate_device");
    if (!id) {
      const bytes = new Uint8Array(32);
      crypto.getRandomValues(bytes);
      id = [...bytes].map((b) => b.toString(16).padStart(2, "0")).join("");
      localStorage.setItem("krate_device", id);
    }
    return id;
  } catch (e) {
    return "";
  }
}

/* Sessions live on the hub, keyed to the account, so a person's work
 * follows them between machines -- something the desktop cannot do. Held
 * in local storage as well so the page is not blank while the hub answers. */
function localSessions() {
  try { return JSON.parse(localStorage.getItem("krate-sessions") || "[]"); } catch (e) { return []; }
}
function saveLocalSessions(list) {
  try { localStorage.setItem("krate-sessions", JSON.stringify(list.slice(0, 60))); } catch (e) {}
}

/* A refusal Studio's UI already knows how to show. The words matter more
 * than the mechanism: they must say what to do next, and never blame the
 * person for standing in a browser. */
function refuse(message) {
  return Promise.reject(new Error(message));
}

/* Hand the finished app to the browser as a download.
 *
 * The build service reads the sign-in from a header, so a plain link to the
 * file answers 401 -- both surfaces used to set `location.href` to it and
 * nothing happened. The bytes are fetched with the token and handed over
 * as an object URL under the app's own file name. */
async function downloadApp(url, fileName) {
  const headers = {};
  if (bridge.token) headers.authorization = `Bearer ${bridge.token}`;
  const res = await fetch(url, { headers });
  if (!res.ok) throw new Error((await res.text().catch(() => "")) || "the file is not there any more; make it again");
  const blob = await res.blob();
  const link = document.createElement("a");
  link.href = URL.createObjectURL(blob);
  link.download = fileName || "app.krate";
  document.body.appendChild(link);
  link.click();
  link.remove();
  setTimeout(() => URL.revokeObjectURL(link.href), 60_000);
}

/* The current app's URL and name, from the session Studio is showing or
 * the last build this page watched. */
function currentWebApp(path) {
  let url = String(path || (bridge.jobResult && `${BUILDER}${bridge.jobResult.download}`) || "");
  let name = (bridge.jobResult && bridge.jobResult.name) ? `${bridge.jobResult.name}.krate` : "";
  if (!/^https?:/.test(url)) {
    // Nothing in memory: this tab did not do the build, or it was
    // reloaded. The sessions carry the same URL, so an app that is on
    // screen is never refused as "not made here" (K-366).
    const saved = localSessions()
      .filter((s) => s && s.result && /^https?:/.test(String(s.result.path || "")))
      .sort((a, b) => (b.updated || 0) - (a.updated || 0))[0];
    if (!saved) return null;
    url = String(saved.result.path);
    name = name || String(saved.result.name || "");
  }
  return { url, name: name || "app.krate" };
}

/* Publish an app the build service holds: the same door `krate publish`
 * uses, fed the same way. The hub wants the bundle bytes as the body and
 * the listing's words in headers -- the bridge used to post a JSON object
 * of file paths, which the hub read as a bundle and refused. The bytes are
 * fetched from the build service with the sign-in, then posted. A picture
 * of the app (the build's own shot, a data URL) goes up beside it the way
 * the CLI's --shot does. Resolves with the app's public URL. */
async function publishWebApp({ path, name, description, shot, unlisted } = {}, token) {
  const app = currentWebApp(path);
  if (!app) throw new Error("This app is not on the build service; publish it from Studio on your computer.");
  const headers = {};
  if (token) headers.authorization = `Bearer ${token}`;
  const got = await fetch(app.url, { headers });
  if (!got.ok) throw new Error((await got.text().catch(() => "")) || "the file is not there any more; make it again");
  const bytes = await got.arrayBuffer();
  const publishHeaders = { ...headers, "content-type": "application/octet-stream" };
  const title = String(name || app.name.replace(/\.krate$/, "") || "").trim();
  if (title) publishHeaders["x-krate-name"] = title;
  if (description) publishHeaders["x-krate-description"] = String(description).slice(0, 500);
  if (unlisted) publishHeaders["x-krate-unlisted"] = "1";
  const res = await fetch(`${HUB}/publish`, { method: "POST", headers: publishHeaders, body: bytes });
  if (!res.ok) throw new Error((await res.text().catch(() => "")) || res.statusText);
  const out = await res.json();
  // The picture is a courtesy: a listing without one is still published.
  if (out.id && shot && /^data:image\/png;base64,/.test(String(shot))) {
    try {
      const png = Uint8Array.from(atob(String(shot).split(",")[1]), (c) => c.charCodeAt(0));
      await fetch(`${HUB}/shot/${out.id}`, { method: "POST", headers: { ...headers, "content-type": "image/png" }, body: png });
    } catch (e) {}
  }
  return out.full_url || out.url;
}

/* ---- the source, handed over ------------------------------------------
 *
 * Every .krate carries the Cargo project that made it -- Cargo.toml,
 * Cargo.lock, manifest.toml, src/ -- because a Krate app is meant to be
 * something you can keep and change, not a black box. On a desktop the
 * Source button opens that folder. In a browser it refused, which for a
 * developer audience is the difference between a toy and a tool.
 *
 * There is nothing to add server-side: the bytes are already on the build
 * service and the source is already inside them. This reads the archive in
 * the tab, pulls out the `source/` entries, and hands back a plain folder
 * as a zip.
 *
 * Enough of the zip format to list the central directory and inflate one
 * entry, mirroring cloud/worker/src/index.js so there is one shape to
 * learn. Deflate uses the browser's own DecompressionStream -- no library,
 * nothing to keep up to date.
 */
function zipU16(b, i) { return b[i] | (b[i + 1] << 8); }
function zipU32(b, i) { return (b[i] | (b[i + 1] << 8) | (b[i + 2] << 16) | (b[i + 3] << 24)) >>> 0; }

function zipEntries(bytes) {
  // EOCD: signature 06054b50, at least 22 bytes, comment up to 65535.
  const floor = Math.max(0, bytes.length - 22 - 65535);
  let eocd = -1;
  for (let i = bytes.length - 22; i >= floor; i -= 1) {
    if (bytes[i] === 0x50 && bytes[i + 1] === 0x4b && bytes[i + 2] === 0x05 && bytes[i + 3] === 0x06) {
      eocd = i;
      break;
    }
  }
  if (eocd < 0) return [];
  const count = zipU16(bytes, eocd + 10);
  let at = zipU32(bytes, eocd + 16);
  const out = [];
  const dec = new TextDecoder();
  for (let n = 0; n < count && at + 46 <= bytes.length; n += 1) {
    if (zipU32(bytes, at) !== 0x02014b50) break;
    const method = zipU16(bytes, at + 10);
    const compressed = zipU32(bytes, at + 20);
    const size = zipU32(bytes, at + 24);
    const nameLen = zipU16(bytes, at + 28);
    const extraLen = zipU16(bytes, at + 30);
    const commentLen = zipU16(bytes, at + 32);
    const local = zipU32(bytes, at + 42);
    out.push({
      name: dec.decode(bytes.subarray(at + 46, at + 46 + nameLen)),
      method, compressed, size, local,
    });
    at += 46 + nameLen + extraLen + commentLen;
  }
  return out;
}

/// The bytes of one entry, inflated when it is deflated. Returns null for
/// anything this reader does not understand rather than guessing.
async function zipRead(bytes, entry) {
  if (entry.local + 30 > bytes.length) return null;
  if (zipU32(bytes, entry.local) !== 0x04034b50) return null;
  const nameLen = zipU16(bytes, entry.local + 26);
  const extraLen = zipU16(bytes, entry.local + 28);
  const from = entry.local + 30 + nameLen + extraLen;
  const raw = bytes.subarray(from, from + (entry.method === 0 ? entry.size : entry.compressed));
  if (entry.method === 0) return raw;
  if (entry.method !== 8) return null;
  const stream = new Blob([raw]).stream().pipeThrough(new DecompressionStream("deflate-raw"));
  return new Uint8Array(await new Response(stream).arrayBuffer());
}

/// A minimal STORED zip, written by hand.
///
/// Stored, not deflated: source files are small, the saving is irrelevant
/// next to the .krate they came from, and a compressor here would be a
/// second implementation of something the platform only gives us one
/// direction of.
function zipWrite(files) {
  const enc = new TextEncoder();
  const chunks = [];
  const central = [];
  let offset = 0;
  // CRC-32, the one piece a zip cannot be written without.
  const table = new Uint32Array(256);
  for (let n = 0; n < 256; n += 1) {
    let c = n;
    for (let k = 0; k < 8; k += 1) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    table[n] = c >>> 0;
  }
  const crc32 = (buf) => {
    let c = 0xffffffff;
    for (let i = 0; i < buf.length; i += 1) c = table[(c ^ buf[i]) & 0xff] ^ (c >>> 8);
    return (c ^ 0xffffffff) >>> 0;
  };
  const put32 = (view, at, v) => { view.setUint32(at, v, true); };
  const put16 = (view, at, v) => { view.setUint16(at, v, true); };

  for (const [name, body] of files) {
    const nameBytes = enc.encode(name);
    const crc = crc32(body);
    const local = new Uint8Array(30 + nameBytes.length);
    const lv = new DataView(local.buffer);
    put32(lv, 0, 0x04034b50);
    put16(lv, 4, 20);          // version needed
    put16(lv, 8, 0);           // stored
    put32(lv, 14, crc);
    put32(lv, 18, body.length);
    put32(lv, 22, body.length);
    put16(lv, 26, nameBytes.length);
    local.set(nameBytes, 30);
    chunks.push(local, body);

    const dir = new Uint8Array(46 + nameBytes.length);
    const dv = new DataView(dir.buffer);
    put32(dv, 0, 0x02014b50);
    put16(dv, 4, 20);
    put16(dv, 6, 20);
    put16(dv, 10, 0);
    put32(dv, 16, crc);
    put32(dv, 20, body.length);
    put32(dv, 24, body.length);
    put16(dv, 28, nameBytes.length);
    put32(dv, 42, offset);
    dir.set(nameBytes, 46);
    central.push(dir);
    offset += local.length + body.length;
  }

  let centralSize = 0;
  for (const d of central) centralSize += d.length;
  const end = new Uint8Array(22);
  const ev = new DataView(end.buffer);
  put32(ev, 0, 0x06054b50);
  put16(ev, 8, central.length);
  put16(ev, 10, central.length);
  put32(ev, 12, centralSize);
  put32(ev, 16, offset);
  return new Blob([...chunks, ...central, end], { type: "application/zip" });
}

/* The Cargo project inside an app, as a download. */
async function downloadSource(path, appName) {
  const app = currentWebApp(path);
  if (!app) throw new Error("This app was not made here, so its source is not here either.");
  const headers = {};
  if (bridge.token) headers.authorization = `Bearer ${bridge.token}`;
  const res = await fetch(app.url, { headers });
  if (!res.ok) throw new Error((await res.text().catch(() => "")) || "the file is not there any more; make it again");
  const bytes = new Uint8Array(await res.arrayBuffer());

  const wanted = zipEntries(bytes).filter((e) => e.name.startsWith("source/") && !e.name.endsWith("/"));
  if (!wanted.length) throw new Error("this app does not carry its source");
  const files = [];
  for (const entry of wanted) {
    const body = await zipRead(bytes, entry);
    // An entry this reader cannot inflate is skipped, not guessed at: a
    // source folder missing one file is worth having and saying so; a
    // folder with one corrupt file in it is not.
    if (body) files.push([entry.name.slice("source/".length), body]);
  }
  if (!files.length) throw new Error("this app's source could not be read");

  const stem = (appName || "app").replace(/\.krate$/, "").replace(/[^a-z0-9]+/gi, "-").toLowerCase();
  const link = document.createElement("a");
  link.href = URL.createObjectURL(zipWrite(files));
  link.download = `${stem}-source.zip`;
  document.body.appendChild(link);
  link.click();
  link.remove();
  setTimeout(() => URL.revokeObjectURL(link.href), 60_000);
  return files.length;
}

/* The build id inside an app's URL, `${BUILDER}/build/<id>/file`. */
function jobIdOf(path) {
  const m = String(path || "").match(/\/build\/([0-9a-f]{32})\/file$/);
  return m ? m[1] : null;
}

/* Watch one job on the build service until it ends, feeding Studio's own
 * build card as it goes and resolving with the shape `create_app` returns
 * on a desktop. Studio's UI drives its stages by reading the engine's
 * lines (`onEngineLine`, the same handler the desktop feeds from its
 * "engine-line" event), so feeding it those lines makes the same card
 * move the same way; the shot goes through the door the desktop's
 * "build-shot" event uses. */
function watchJob(jobId, request, sessionId) {
  bridge.job = jobId;
  bridge.jobResult = null;
  let lastLine = "";
  let lastShot = "";
  return new Promise((resolve, reject) => {
    const tick = async () => {
      let job;
      try {
        job = await builder(`/build/${jobId}`);
      } catch (err) {
        clearInterval(bridge.poll);
        return reject(err);
      }
      if (job.line && job.line !== lastLine && typeof window.onEngineLine === "function") {
        lastLine = job.line;
        window.onEngineLine(job.line);
      }
      if (job.shot && job.shot !== lastShot && typeof window.onBuildShot === "function") {
        lastShot = job.shot;
        window.onBuildShot(job.shot);
      }
      if (job.state === "done") {
        clearInterval(bridge.poll);
        bridge.jobResult = job.result;
        const result = {
          path: `${BUILDER}${job.result.download}`,
          name: `${job.result.name}.krate`,
          size: job.result.size,
          asks: job.result.asks || [],
          shot: job.result.shot || "",
          // The build service's request verdict, in the desktop's own
          // field names, so the one done card reads both.
          verdict: job.result.verdict || null,
          verdict_detail: job.result.verdict_detail || null,
          // Made on the web: the desktop fetches the file from this URL
          // with the same sign-in before it opens or changes the app.
          web: true,
        };
        if (sessionId) {
          // The session saved before the build now carries the app, so the
          // wall's promise ("this session is waiting in Studio") is kept
          // with the file, not only the sentence they typed. The record is
          // updated in place: what Studio's UI wrote there (messages, the
          // build count, the plan) stays, and only the result is new.
          const existing = localSessions().find((s) => s.id === sessionId);
          const saved = existing
            ? { ...existing, updated: Date.now(), result }
            : {
              id: sessionId,
              title: (request || "").slice(0, 80),
              created: Date.now(),
              updated: Date.now(),
              messages: [
                { who: "YOU", body: request || "" },
                { who: "KRATE", body: `Made ${result.name} (${result.size}).` },
              ],
              result,
            };
          COMMANDS.session_save({ session: saved }).catch(() => {});
        }
        return resolve(result);
      }
      if (job.state === "failed" || job.state === "stopped") {
        clearInterval(bridge.poll);
        return reject(new Error(job.error || "that build stopped"));
      }
    };
    bridge.poll = setInterval(tick, 1500);
    tick();
  });
}

const COMMANDS = {
  /* ---- who they are ---------------------------------------------------- */

  async account_status() {
    if (!bridge.token) return { signed_in: false };
    try {
      bridge.me = await hub("/me");
      const u = bridge.me.user || {};
      return { signed_in: true, login: u.login, name: u.name, avatar_url: u.avatar_url, email: u.email };
    } catch (e) {
      bridge.token = null;
      try { localStorage.removeItem(TOKEN_KEY); } catch (e2) {}
      return { signed_in: false };
    }
  },

  /* ---- your own AI ------------------------------------------------------
   *
   * Studio's key sheet is already built and already speaks these three
   * commands; on a desktop they reach the OS keychain. A browser has no
   * keychain, so the hub holds the key instead -- encrypted, and never
   * handed back to this page. What comes back is whether a key is set and
   * its last four characters, which is enough to recognise it.
   *
   * The bargain: Krate pays for the first app in a tab. After that, bring a
   * key and keep building here, or move to Studio where your own AI is
   * already installed.
   */
  async api_keys() {
    if (!bridge.token) return [];
    const out = await hub("/keys");
    return (out.keys || []).map((k) => ({
      vendor: k.vendor,
      label: k.label,
      set: Boolean(k.set),
      // Studio shows this where the desktop shows "macOS keychain".
      where_kept: k.set ? `saved to your account${k.tail ? ` -- ends ${k.tail}` : ""}` : "",
      from_env: false,
    }));
  },

  async api_key_set({ vendor, key } = {}) {
    if (!bridge.token) return refuse("Sign in first.");
    await hub("/keys", { method: "POST", body: JSON.stringify({ vendor, key }) });
    return "Saved. Your builds here now run on your key.";
  },

  async api_key_forget({ vendor } = {}) {
    if (!bridge.token) return refuse("Sign in first.");
    await hub("/keys/forget", { method: "POST", body: JSON.stringify({ vendor }) });
    return "Removed.";
  },

  /* What the builds have cost, from the engine's own token counts. Two
   * buckets because they are different money: what Krate funded, and what
   * came off your own key. */
  async spend_report() {
    if (!bridge.token) return { builds: 0, total: { own: 0, krate: 0 }, recent: [] };
    return hub("/spend");
  },

  async me_info() {
    if (!bridge.me) await COMMANDS.account_status();
    return bridge.me || {};
  },

  login_browser() {
    // The site's own sign-in, which knows how to come back here: /login
    // remembers `next` and /login/done honours it.
    location.href = "/login/?next=studio";
    return Promise.resolve();
  },

  account_login() {
    return COMMANDS.login_browser();
  },

  account_logout() {
    bridge.token = null;
    bridge.me = null;
    try { localStorage.removeItem(TOKEN_KEY); } catch (e) {}
    return Promise.resolve();
  },

  /* ---- the plan and the money ------------------------------------------ */

  async plan_makes() {
    try {
      const out = await hub("/plan/get", {
        method: "POST",
        body: JSON.stringify({ device: deviceId() }),
      });
      return out.n || 0;
    } catch (e) { return 0; }
  },

  async plan_count_make() {
    // Counting is the builder's job, not the browser's -- a page that
    // counts its own makes is a page anyone can edit. This only reads back
    // what the server already recorded.
    return COMMANDS.plan_makes();
  },

  async billing_info() {
    if (!bridge.me) await COMMANDS.account_status();
    const plan = (bridge.me && bridge.me.plan) || {};
    return { plan: plan.plan || "free", active: Boolean(plan.active), until: plan.until || 0, portal: Boolean(plan.portal) };
  },

  async billing_checkout({ plan } = {}) {
    const out = await hub("/billing/checkout", {
      method: "POST",
      body: JSON.stringify({ plan, return_url: location.href }),
    });
    if (out.url) location.href = out.url;
    return out;
  },

  /* ---- making ----------------------------------------------------------
   * The one command that needed a service behind it. Everything the build
   * card shows -- stage, line, the app's own first frame -- comes from the
   * builder, which parses the same engine output Studio parses locally.
   */

  async create_app({ request, session, starterShape } = {}) {
    // Plan mode means plan mode. Studio's chat has an escape hatch -- typing
    // "build it" during planning starts the build -- and on a desktop that
    // is right, because the person chose to plan in that moment. Here they
    // chose a MODE, before typing, and a mode that sometimes builds is not
    // a mode. The refusal says how to change it.
    if (webMode() === "plan") {
      return refuse("You are in Plan mode, so nothing is built. Switch to Build in the box below to make this app.");
    }
    // Nobody makes an app without an account: it is how the funded first
    // app is counted, and how the work belongs to someone. But the
    // sentence they just typed must survive the round trip -- being asked
    // to remember and retype it is the moment a person decides the
    // product is careless.
    if (!bridge.token) {
      try { localStorage.setItem(PENDING_KEY, request || ""); } catch (e) {}
      await COMMANDS.login_browser();
      return new Promise(() => {});  // the page is navigating away
    }
    // The session is saved BEFORE the build starts, so the wall below has
    // something true to point at. Being told "your session is waiting in
    // Studio" and then finding nothing there is worse than no wall at all.
    // Studio's UI names the session it is building in (`session`), and
    // that record is the one that counts: it already holds the plan, the
    // messages and the build count. The bridge used to write a second
    // record of its own beside it, so every web build left a duplicate
    // "Draft" in the sidebar, and a change made on the duplicate started
    // its numbering again at v1.
    const sessionId = session || `web-${Date.now()}`;
    if (!localSessions().some((s) => s.id === sessionId)) {
      await COMMANDS.session_save({
        session: {
          id: sessionId,
          title: (request || "").slice(0, 80),
          created: Date.now(),
          updated: Date.now(),
          messages: [{ who: "YOU", body: request || "" }],
          result: null,
        },
      }).catch(() => {});
    }

    let started;
    try {
      started = await builder("/build", {
        method: "POST",
        body: JSON.stringify({ request, device: deviceId(), shape: starterShape || "" }),
      });
    } catch (err) {
      // The one-app wall. The builder answers with `download: true` when the
      // right next step is the desktop rather than a payment, and Studio's UI
      // reads `err.download` to offer that instead of a plain failure card.
      const text = String((err && err.message) || err || "");
      let parsed = null;
      try { parsed = JSON.parse(text); } catch (_) {}
      if (parsed && parsed.wall) {
        const wall = new Error(parsed.message || "You have made your app.");
        wall.wall = true;
        wall.download = Boolean(parsed.download);
        throw wall;
      }
      throw err;
    }
    return watchJob(started.id, request, sessionId);
  },

  /* A change to the app they made, in the same funded case: the build
   * service runs `krate revise` on the file it made, exactly as Studio on
   * a desktop runs it on the file in your folder. The app's URL names the
   * build it came from; the service checks that build is theirs. */
  async revise_app({ path, change } = {}) {
    if (!bridge.token) return refuse("Sign in to change your app.");
    const id = jobIdOf(path);
    if (!id) return refuse("This app was not made here, so it cannot be changed here. Open it in Studio on your computer.");
    let started;
    try {
      started = await builder(`/build/${id}/revise`, {
        method: "POST",
        body: JSON.stringify({ change, device: deviceId() }),
      });
    } catch (err) {
      const text = String((err && err.message) || err || "");
      let parsed = null;
      try { parsed = JSON.parse(text); } catch (_) {}
      if (parsed && parsed.wall) {
        const wall = new Error(parsed.message || "You have made your app.");
        wall.wall = true;
        wall.download = Boolean(parsed.download);
        throw wall;
      }
      throw err;
    }
    return watchJob(started.id, change, null);
  },

  /* The conversation before a build: the engine's own `plan` step, on the
   * build service, so the questions and the plan are the same ones Studio
   * asks on a desktop. Attachments are not carried to the web yet, and the
   * plan says so rather than pretending to have read them. */
  async plan_request({ request, attachments } = {}) {
    if (!bridge.token) return refuse("Sign in first.");
    if (attachments && attachments.length) {
      return refuse("Attaching files is coming to the web version; the plan is made from your words alone.");
    }
    const answer = await builder("/plan", {
      method: "POST",
      body: JSON.stringify({ request, device: deviceId() }),
    });
    // Studio's UI expects the engine's JSON as text, exactly as the
    // desktop hands it over.
    return typeof answer === "string" ? answer : JSON.stringify(answer);
  },


  async build_alive() {
    return Boolean(bridge.job);
  },

  async stop_build() {
    if (!bridge.job) return;
    clearInterval(bridge.poll);
    await builder(`/build/${bridge.job}/stop`, { method: "POST" }).catch(() => {});
    bridge.job = null;
  },

  /* ---- their work ------------------------------------------------------- */

  /* Sessions live on the hub when there is an account, and in localStorage
   * when there is not.
   *
   * The point of the hub copy is the hand-off: somebody makes an app here,
   * downloads Studio, signs in, and the conversation is waiting -- editable,
   * not a read-only receipt. localStorage cannot do that, and it is also
   * one cleared cache away from losing the work.
   *
   * Local stays as the cache and the signed-out path, and the two are merged
   * on read by `updated` so a session made before signing in is not lost the
   * moment an account appears.
   */
  async sessions_list() {
    const local = localSessions();
    if (!bridge.token) return local;
    let remote = [];
    try {
      remote = (await hub("/sessions")).sessions || [];
    } catch (e) {
      // Offline, or the hub is down. Their local copy is still their work.
      return local;
    }
    const byId = new Map();
    for (const s of [...local, ...remote]) {
      const seen = byId.get(s.id);
      if (!seen || (s.updated || 0) > (seen.updated || 0)) byId.set(s.id, s);
    }
    const merged = [...byId.values()].sort((a, b) => (b.updated || 0) - (a.updated || 0));
    saveLocalSessions(merged);
    return merged;
  },

  async session_save({ session } = {}) {
    // Local first, always. The hub write can fail -- no signal, expired
    // token -- and losing what somebody just typed because a network call
    // did not land is the one outcome worth ruling out entirely.
    const list = localSessions().filter((s) => s.id !== session.id);
    list.unshift(session);
    saveLocalSessions(list);
    if (!bridge.token) return;
    try {
      await hub("/sessions", { method: "POST", body: JSON.stringify(session) });
    } catch (e) {
      // Kept locally; the next successful list merges it up.
    }
  },

  async session_delete({ id } = {}) {
    saveLocalSessions(localSessions().filter((s) => s.id !== id));
    if (!bridge.token) return;
    try {
      await hub(`/sessions/${encodeURIComponent(id)}`, { method: "DELETE" });
    } catch (e) {}
  },

  async session_shot({ id } = {}) {
    const found = localSessions().find((s) => s.id === id);
    return (found && found.result && found.result.shot) || "";
  },

  /* ---- sharing ---------------------------------------------------------- */

  async publish(args = {}) {
    if (!bridge.token) return refuse("Sign in to publish.");
    // Studio hands over the picture it chose as a path; on the web that
    // path is the build's own shot (a data URL) or nothing.
    const shot = args.shot || (bridge.jobResult && bridge.jobResult.shot) || "";
    return publishWebApp({ ...args, shot }, bridge.token);
  },

  async open_external({ url } = {}) {
    window.open(url, "_blank", "noopener");
  },

  /* ---- what a tab cannot do --------------------------------------------
   * Each of these refuses in words that say what to do instead. A browser
   * cannot open a native window, reach a folder, or install a command --
   * and a person hearing that plainly is better served than one clicking a
   * button that does nothing.
   */

  /* Opening, on the web, is downloading: the file is the product, and it
   * opens on the person's own computer. The button does the useful thing
   * and then says where the app really runs. */
  async open_app({ path } = {}) {
    const app = currentWebApp(path);
    if (!app) return refuse("A browser cannot open the app itself. Download the file -- it opens on your Mac, Windows or Linux.");
    await downloadApp(app.url, app.name);
    return refuse("Downloaded. Double-click the file on your Mac, Windows or Linux -- that is where the app really runs.");
  },
  async open_krate({ path } = {}) {
    return COMMANDS.open_app({ path });
  },
  async autorun({ path } = {}) {
    return COMMANDS.open_app({ path });
  },
  pick_folder() {
    return refuse("A browser chooses where downloads go, not this page.");
  },
  pick_files() {
    return refuse("Attaching files is coming to the web version.");
  },
  pick_image() {
    return refuse("Attaching a picture is coming to the web version.");
  },
  install_agent() {
    return refuse("The web version uses our AI, so there is nothing to install.");
  },
  link_terminal_tool() {
    return refuse("That is a desktop thing. Download Studio if you want the terminal command.");
  },
  make_wrap() {
    return refuse("The gift for a friend without Krate is made in Studio on your computer.");
  },
  make_card() {
    return refuse("The card is made in Studio on your computer -- download the file and open it there. Send a link works from here.");
  },
  share_file() {
    return refuse("Download the file; sharing it from this page is not built yet.");
  },
  async read_image({ path } = {}) {
    // A picture that is already a URL is its own data.
    if (/^(https?:|data:)/.test(String(path || ""))) return path;
    return refuse("That picture lives on your computer, not on this page.");
  },
  /* Studio asks for a folder to reveal; a browser has no folders, so the
   * honest equivalent is the project itself, handed over. Returning a
   * non-empty string keeps the UI's Source button enabled -- it checks for
   * one before offering the button -- and `reveal` below does the work. */
  async session_source_dir() {
    return currentWebApp(null) ? "source" : "";
  },
  async reveal({ path } = {}) {
    // Studio's Source button reveals the folder session_source_dir gave it.
    // In a tab that is the marker above, and revealing it means handing the
    // Cargo project over.
    if (path === "source") {
      const name = (bridge.jobResult && bridge.jobResult.name) || "app";
      const n = await downloadSource(null, name);
      return refuse(`Downloaded the project -- ${n} file${n === 1 ? "" : "s"}. Open the folder with cargo, or in Studio on your computer.`);
    }
    const app = currentWebApp(path);
    if (!app) return refuse("Check your downloads folder.");
    await downloadApp(app.url, app.name);
    return refuse("Downloaded -- check your downloads folder.");
  },

  /* ---- the quiet ones ---------------------------------------------------
   * Commands whose honest browser answer is simply "nothing to do". These
   * must NOT refuse: a rejection here would surface an error over a
   * courtesy the person never asked for.
   */

  async agents() {
    // One AI, ours, always ready. The chip says so and the picker is moot.
    return [{ name: "krate", label: "Krate AI", state: "working", detail: "", remedy: null }];
  },
  async refresh_agents() { return COMMANDS.agents(); },
  async settings_get() { return { out_dir: "", agent: "krate" }; },
  async settings_set() {},
  async studio_version() { return "web"; },
  // The engine is the build service's, and the service is never older than
  // the page that was deployed with it.
  async engine_status() { return { path: BUILDER, version: "web", studio_version: "web", lags: false }; },
  async build_progress() {},
  async dbg_log() {},
  async usage_flush() {},
  async win_minimize() {},
  async win_toggle_max() {},
  async win_close() {},
  async latest_release() { return null; },
  async install_update() {},
  async restart_for_update() {},
  async first_run_setup() {},
};

/* The door itself. Anything not named above is a command the browser has
 * no answer for; saying so is better than a silent undefined that shows up
 * three screens later as an empty panel. */
window.__KRATE_BRIDGE__ = async function bridgeInvoke(cmd, args) {
  const fn = COMMANDS[cmd];
  if (!fn) {
    console.warn(`krate: no browser answer for "${cmd}"`);
    return refuse("That part of Studio needs the app on your computer.");
  }
  return fn(args || {});
};

/* The handful of sentences that are true on a desktop and wrong in a tab.
 *
 * Studio says "an AI you already have", because on a desktop it drives the
 * Claude or Codex the person installed. Here it is ours, and telling
 * someone to bring their own AI to a website they are already using would
 * be the one confusing moment in an otherwise honest flow.
 *
 * Kept to a short, explicit list rather than a rewrite: anything not named
 * here is Studio's own copy, unchanged, which is the point of reusing it.
 */
/* The sentence they typed before signing in, put back in the box.
 *
 * Restored rather than auto-submitted: they are landing on a screen they
 * have not seen before, and a build starting by itself would take the
 * decision away at the exact moment they are getting their bearings. The
 * words are there, the cursor is at the end, and the next move is theirs.
 */
function restorePending() {
  let pending = "";
  try {
    pending = localStorage.getItem(PENDING_KEY) || "";
    localStorage.removeItem(PENDING_KEY);
  } catch (e) {}
  if (!pending) return;
  const box = document.getElementById("homePrompt");
  if (!box) return;
  box.value = pending;
  box.dispatchEvent(new Event("input"));
  box.focus();
  // Size it once the box is actually on screen. Measuring a hidden element
  // gives a scrollHeight that is not the text's, and the box ends up
  // wearing its full 132px cap for one short line -- which looks broken in
  // a way that is hard to trace back to a restore that otherwise worked.
  requestAnimationFrame(() => {
    box.style.height = "auto";
    box.style.height = Math.min(box.scrollHeight, 132) + "px";
    try { box.setSelectionRange(box.value.length, box.value.length); } catch (e) {}
  });
  try { box.setSelectionRange(box.value.length, box.value.length); } catch (e) {}
}

/* ---- what you have spent ------------------------------------------------
 *
 * Anybody paying for their own inference is owed the number, per build, not
 * a monthly total they cannot check. The engine prices every run from the
 * API's own token counts; the hub keeps the ledger; this draws it.
 *
 * Two columns because it is two different pockets: what Krate funded, and
 * what came off the person's own key. A single total would hide exactly the
 * thing somebody with a key wants to see.
 *
 * Built here rather than in Studio's HTML because only the browser has this
 * problem: on a desktop the key is in the keychain and the spend is on the
 * person's own API bill, where their provider already shows it.
 */
const SPEND_CSS = `
.web-spend { margin-top: 22px; }
.web-spend h3 { font-size: 13px; font-weight: 600; margin: 0 0 4px; }
.web-spend .set-sub { margin-bottom: 12px; }
.web-spend-cards { display: flex; gap: 10px; flex-wrap: wrap; margin-bottom: 14px; }
.web-spend-card {
  flex: 1 1 130px; padding: 11px 13px; border-radius: 11px;
  border: 1px solid var(--line); background: var(--film);
}
.web-spend-card .n { font-size: 19px; font-weight: 600; font-variant-numeric: tabular-nums; }
.web-spend-card .k { font-size: 11px; color: var(--muted); margin-top: 2px; }
.web-spend-rows { border-top: 1px solid var(--line); }
.web-spend-row {
  display: flex; align-items: baseline; gap: 10px;
  padding: 7px 2px; border-bottom: 1px solid var(--line); font-size: 12px;
}
.web-spend-row .app { flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.web-spend-row .who { font-size: 10.5px; color: var(--muted); }
.web-spend-row .usd { font-variant-numeric: tabular-nums; }
.web-spend-empty { font-size: 12px; color: var(--muted); padding: 10px 0; }
`;

function money(n) {
  const v = Number(n) || 0;
  // Under a cent is still a number somebody can check, and "$0.00" next to
  // a real build reads as broken rather than cheap.
  if (v > 0 && v < 0.01) return "<$0.01";
  return `$${v.toFixed(2)}`;
}

async function paintSpend() {
  const pane = document.querySelector('.ai-pane[data-ai="keys"]');
  if (!pane) return;
  let box = document.getElementById("webSpend");
  if (!box) {
    box = document.createElement("div");
    box.className = "web-spend";
    box.id = "webSpend";
    pane.appendChild(box);
  }
  let report;
  try {
    report = await COMMANDS.spend_report();
  } catch (e) {
    box.innerHTML = '<p class="web-spend-empty">Could not read your usage just now.</p>';
    return;
  }
  const rows = (report.recent || []).map((r) => {
    const when = r.at ? new Date(r.at * 1000).toLocaleDateString() : "";
    const who = r.paid_by === "own" ? "your key" : "on Krate";
    const app = (r.app || "an app").replace(/[<>]/g, "");
    const model = (r.model || "").replace(/[<>]/g, "");
    return `<div class="web-spend-row">
      <span class="app">${app}</span>
      <span class="who">${who}${model ? ` -- ${model}` : ""}${r.rounds ? ` -- ${r.rounds} rounds` : ""}</span>
      <span class="who">${when}</span>
      <span class="usd">${money(r.usd)}</span>
    </div>`;
  }).join("");
  const t = report.total || { own: 0, krate: 0 };
  const m = report.month || { own: 0, krate: 0 };
  box.innerHTML = `
    <h3>What you have spent</h3>
    <p class="set-sub">Priced from the model's own token counts, per build.
    Krate paid for your first app; anything on your key is billed by your
    provider, not by us.</p>
    <div class="web-spend-cards">
      <div class="web-spend-card"><div class="n">${money(t.own)}</div><div class="k">your key, all time</div></div>
      <div class="web-spend-card"><div class="n">${money(m.own)}</div><div class="k">your key, 30 days</div></div>
      <div class="web-spend-card"><div class="n">${money(t.krate)}</div><div class="k">funded by Krate</div></div>
      <div class="web-spend-card"><div class="n">${report.builds || 0}</div><div class="k">builds</div></div>
    </div>
    ${rows
      ? `<div class="web-spend-rows">${rows}</div>`
      : '<p class="web-spend-empty">Nothing yet. Your first build will show up here with what it cost.</p>'}
  `;
}

/* ---- Plan or Build, chosen before you type ------------------------------
 *
 * Studio's desktop flow always plans first: the request is looked at, a
 * question or a plan comes back, and the build starts after that. Good for
 * somebody who has used it before; on the web it means a new person types a
 * sentence and gets a conversation when they expected an app.
 *
 * So the browser asks once, in the composer, before anything is typed:
 *
 *   Build  -- the default. Up to three questions when the answer would
 *             change what gets built, then it builds. (Today's behaviour.)
 *   Plan   -- a plan and nothing else. No build, no file, no spend beyond
 *             the one short answer.
 *
 * Plan mode is a promise: choosing it must never produce an app. That is
 * enforced in `create_app` below, not only in the UI, because a stray
 * "build it" in the chat would otherwise walk straight past the choice.
 */
const MODE_KEY = "krate_web_mode";
function webMode() {
  try { return localStorage.getItem(MODE_KEY) === "plan" ? "plan" : "build"; } catch (e) { return "build"; }
}
function setWebMode(mode) {
  try { localStorage.setItem(MODE_KEY, mode === "plan" ? "plan" : "build"); } catch (e) {}
  paintMode();
}

const MODE_CSS = `
.web-mode { display: inline-flex; align-items: center; gap: 2px;
  padding: 2px; border-radius: 999px; background: var(--film);
  border: 1px solid var(--line); margin-right: 8px; }
.web-mode button {
  appearance: none; border: 0; background: none; cursor: pointer;
  font: inherit; font-size: 11.5px; font-weight: 500; line-height: 1;
  color: var(--muted); padding: 5px 11px; border-radius: 999px;
  transition: background 0.15s, color 0.15s;
}
.web-mode button:hover { color: var(--text); }
.web-mode button[aria-pressed="true"] { background: var(--text); color: var(--bg); }
.web-mode-hint { font-size: 11px; color: var(--muted); margin-left: 2px; }
`;

function paintMode() {
  const mode = webMode();
  document.querySelectorAll(".web-mode button").forEach((b) => {
    b.setAttribute("aria-pressed", String(b.dataset.mode === mode));
  });
  const box = document.getElementById("homePrompt");
  if (box) {
    box.placeholder = mode === "plan"
      ? "Describe an app -- you will get a plan, not a build…"
      : "Describe an app, or paste code to port…";
  }
  const send = document.getElementById("homeSend");
  if (send) send.title = mode === "plan" ? "Plan it (Enter)" : "Make it (Enter)";
}

/* The toggle lives in the composer row, beside the AI chip: the two
 * decisions about a message -- who writes it, and whether this is a plan or
 * a build -- belong in the same place, on the bar you are typing into. */
function mountMode() {
  const row = document.querySelector("#viewHome .bigbar-row");
  if (!row || document.querySelector(".web-mode")) return;
  const wrap = document.createElement("div");
  wrap.className = "web-mode";
  wrap.setAttribute("role", "group");
  wrap.setAttribute("aria-label", "Plan or build");
  wrap.innerHTML = `
    <button type="button" data-mode="build" title="Answer a question or two, then build the app">Build</button>
    <button type="button" data-mode="plan" title="Get a plan only -- nothing is built">Plan</button>`;
  wrap.querySelectorAll("button").forEach((b) => {
    b.addEventListener("click", () => setWebMode(b.dataset.mode));
  });
  const grow = row.querySelector(".grow");
  if (grow) row.insertBefore(wrap, grow.nextSibling);
  else row.appendChild(wrap);
  paintMode();
}

/* ---- what a tab must not be offered -------------------------------------
 *
 * Studio's settings are written for a computer: a folder apps are written
 * to, a terminal to install a command into, a command of your own to author
 * with. In a browser every one of those is either a refusal dressed as a
 * button or, worse, a control that looks live and does nothing.
 *
 * A refusal in plain words is right when somebody ASKED for the thing (Run
 * it, Reveal). It is wrong for a settings row, which nobody asked for and
 * which is read as a list of what this product can do. So these are
 * removed, not disabled: a greyed-out row still says "this exists, but not
 * for you", and that is not true either -- it exists on the desktop, which
 * is one download away and is named in the rows that stay.
 *
 * Removed by the label the person reads, not by an id: a row that gets
 * renamed should stop matching and be re-judged, rather than silently keep
 * hiding the wrong thing.
 */
const DESKTOP_ONLY_ROWS = [
  "Output folder",       // no folder to write to
  "Where your apps are", // the browser's downloads folder is the OS's business
  "Your own command",    // no processes to run
];

function trimDesktopOnly() {
  document.querySelectorAll(".set-row").forEach((row) => {
    const label = row.querySelector(".lab");
    if (label && DESKTOP_ONLY_ROWS.includes(label.textContent.trim())) row.remove();
  });
  // The terminal group ships hidden and is unhidden on a desktop; make sure
  // nothing here ever shows it.
  const term = document.getElementById("setTerminalGroup");
  if (term) term.remove();
  // A settings group whose every row has gone should go too, or the sheet
  // grows headings standing over nothing.
  document.querySelectorAll(".set-group, .set-panel").forEach((group) => {
    if (!group.querySelector(".set-row")) group.remove();
  });
}

function speakWeb() {
  // Nothing to swap in onboarding: a tab never shows it (see the
  // krate-onboarded flag at the top). The desktop's wording is its own.
  // On a desktop, a failed build can be retried with a different AI --
  // that is the point of Studio driving the one you already have. Here
  // there is only ours, so the button would lead to a picker with one
  // entry and no answer. Offering it would be a dead end dressed as help.
  const switchAi = document.getElementById("switchAiBtn");
  if (switchAi) switchAi.remove();
  // Studio checks GitHub for a newer desktop release and offers the
  // download; in a tab there is nothing to update, and "Update to v0.3.0"
  // on a website is a lie about a file that does not exist here. The check
  // itself runs in Studio's code; the chip is simply never shown.
  for (const id of ["updateChip", "dockBadge"]) {
    const el = document.getElementById(id);
    if (el) el.remove();
  }
  try {
    const sheet = document.createElement("style");
    sheet.textContent = SPEND_CSS + MODE_CSS;
    document.head.appendChild(sheet);
  } catch (e) {}
  // Studio paints its home a beat after boot, so the composer may not
  // exist yet when this first runs.
  mountMode();
  setTimeout(mountMode, 400);
  setTimeout(mountMode, 1200);
  trimDesktopOnly();
  // The settings sheet is in the page from the start, but a pane can be
  // painted later; trim again when one is opened.
  document.addEventListener("click", () => setTimeout(trimDesktopOnly, 50), true);
  // Drawn when the AI settings open, because that is where the key lives
  // and the two questions -- whose key, and what has it cost -- are one
  // question. Re-read every time rather than cached: a number about money
  // that is quietly stale is worse than no number.
  document.addEventListener("click", (event) => {
    const opener = event.target.closest('[data-ai], #aiBtn, [data-sheet="ai"]');
    if (opener) setTimeout(paintSpend, 60);
  }, true);

  // Studio paints its home a beat after boot, so the box may not exist
  // yet when this first runs.
  restorePending();
  setTimeout(restorePending, 900);
}
if (document.readyState === "loading") {
  document.addEventListener("DOMContentLoaded", speakWeb);
} else {
  speakWeb();
}

/* Sign in before the Studio, not in the middle of it.
 *
 * Every build in a browser runs on our machine, so an account is required
 * whatever happens. Asking at the moment somebody presses Make meant taking
 * away the sentence they had just written and handing back a login page --
 * the request survives (PENDING_KEY) but the interruption does not need to
 * exist. Arriving signed out now goes straight to the sign-in and comes
 * back to a Studio that is ready to work.
 *
 * `?stay` is the way back in for anyone who deliberately signed out and
 * wants to look around, and /login itself must never bounce: without that
 * guard a failed sign-in would ping-pong between the two pages. */
function requireSignIn() {
  if (bridge.token) return;
  const params = new URLSearchParams(location.search);
  if (params.has("stay")) return;
  location.replace("/login/?next=studio");
}
requireSignIn();

console.info("krate: studio bridge ready (hub + builder)");
