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
/* The sign-in screen, hidden before it can ever be painted.
 *
 * `viewGate` is the one view in Studio's HTML that does not ship with
 * `hidden` on it, because on a desktop it IS the first screen. So the
 * browser painted it on every load, and Studio only swapped it for Home
 * once its script had booted and asked who was signed in -- which is the
 * two to three seconds of sign-in page a signed-in person saw on every
 * refresh, and the first thing somebody saw right after signing in.
 *
 * The token is in local storage and readable synchronously, right here,
 * before the document has been painted once. When it is there, the gate
 * is never shown rather than shown and taken away. Studio's own boot
 * still decides which view to reveal; this only stops the wrong one being
 * visible in the meantime.
 *
 * `visibility` rather than `display`: the rule is dropped again as soon as
 * Studio has chosen a view, and a hidden-then-shown flex column re-runs
 * its layout, which is a flash of its own.
 *
 * WHEN the rule is dropped is the whole trick, and the obvious answers are
 * both wrong. `window.load` fires when the page's own resources are in,
 * which is BEFORE app.js has booted and decided anything -- measured with
 * app.js arriving 1.8s late, the gate was visible for 1,801ms of it. A
 * timer is a guess that is too short on a slow connection and wasted time
 * on a fast one.
 *
 * So this watches for the thing it is actually waiting for: Studio marks
 * the gate `hidden` the moment it knows the person is signed in. The
 * observer lifts the rule then, and a fallback lifts it anyway after a few
 * seconds so a boot that never happens cannot leave a blank screen.
 */
/* While the gate is held back there is nothing else to look at yet, so the
 * page is a dark rectangle until app.js lands. That is already better than
 * a sign-in screen somebody has to watch disappear, but it reads as a
 * page that has stopped. The mark, quietly breathing, says it is coming.
 *
 * Drawn in CSS on a pseudo-element of the gate itself, so it costs no
 * markup and vanishes with the same rule. */
const GATE_CSS = `
  #viewGate { visibility: hidden; }
  #viewGate::after {
    visibility: visible;
    content: "";
    position: fixed;
    left: 50%; top: 50%;
    width: 44px; height: 44px;
    margin: -22px 0 0 -22px;
    border-radius: 12px;
    background: url("krate-logo.png") center / contain no-repeat;
    opacity: 0.55;
    animation: krate-wait 1.4s ease-in-out infinite;
  }
  @keyframes krate-wait {
    0%, 100% { opacity: 0.22; transform: scale(0.96); }
    50%      { opacity: 0.62; transform: scale(1); }
  }
  @media (prefers-reduced-motion: reduce) {
    #viewGate::after { animation: none; opacity: 0.45; }
  }
`;
try {
  const sheet = document.createElement("style");
  sheet.textContent = WEB_CSS;
  document.head.appendChild(sheet);
  if (bridge.token) {
    // Its own sheet, so lifting it later cannot disturb WEB_CSS.
    const gate = document.createElement("style");
    gate.textContent = GATE_CSS;
    document.head.appendChild(gate);
    const lift = () => { try { gate.remove(); } catch (e) {} };
    const watchGate = () => {
      const el = document.getElementById("viewGate");
      if (!el) return false;
      // Already decided (a fast boot beat this code to it).
      if (el.classList.contains("hidden")) { lift(); return true; }
      if (!window.MutationObserver) return false;
      const obs = new MutationObserver(() => {
        if (el.classList.contains("hidden")) { obs.disconnect(); lift(); }
      });
      obs.observe(el, { attributes: true, attributeFilter: ["class"] });
      return true;
    };
    if (!watchGate()) {
      document.addEventListener("DOMContentLoaded", watchGate, { once: true });
    }
    // The gate is the SIGNED-OUT screen and this rule hides it. If Studio
    // never boots -- a script that failed to load, an error on the way up
    // -- lifting it is the difference between a sign-in page and nothing
    // at all. Long enough that a slow connection is not cut short.
    setTimeout(lift, 8000);
  }
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
/* The files somebody attached, by the name Studio shows on the chip.
 *
 * Studio's attachment list is a list of STRINGS, because on a desktop
 * those are paths. A browser has no path to give it, so the string is the
 * file's own name and the bytes live here beside it. Keyed by the exact
 * string handed back from the picker, so removing a chip and re-adding a
 * file of the same name behave the way a person expects.
 */
const attached = new Map();

/// The most one attachment may be. Ten megabytes covers a screenshot, a
/// logo, a CSV or a source file with room to spare; past that the request
/// gets slow to send and the model cannot read it usefully anyway.
const MAX_ATTACH_BYTES = 10 * 1024 * 1024;

/// The longest request the build service will work from.
///
/// It refuses anything past this with "That is longer than we can work
/// from", and it does so in three places (build, plan, revise). Nothing
/// in the page stopped a long paste before now, so 50,000 characters
/// travelled all the way there to come back as a bare failure. Said here
/// instead, instantly, before anything is sent.
const MAX_REQUEST_CHARS = 2000;

/// Refuse a request that is too long, in words that say what to do.
function tooLong(text, what) {
  const n = String(text || "").length;
  if (n <= MAX_REQUEST_CHARS) return null;
  return refuse(
    `That ${what} is ${n.toLocaleString()} characters, and we can work from ` +
    `${MAX_REQUEST_CHARS.toLocaleString()}. Say the shape of what you want ` +
    `in a sentence or two; the AI asks for the rest.`,
  );
}

/* Open the browser's own file picker and read what was chosen.
 *
 * Returns the names, which is what Studio's UI renders. A file too big is
 * refused BY NAME, so a person who attached three things and one was too
 * large is told which one rather than losing all three silently.
 */
function pickLocalFiles({ accept, multiple }) {
  return new Promise((resolve, reject) => {
    const input = document.createElement("input");
    input.type = "file";
    if (accept) input.accept = accept;
    input.multiple = Boolean(multiple);
    input.style.display = "none";
    document.body.appendChild(input);

    // A picker that is cancelled must not hang the promise for ever:
    // Studio awaits this, and an await that never settles leaves the
    // composer waiting on a dialog the person already dismissed. There is
    // no reliable cancel event, so the window regaining focus is the
    // signal, one tick late so a real choice lands first.
    let settled = false;
    const done = (fn, value) => {
      if (settled) return;
      settled = true;
      try { input.remove(); } catch (e) {}
      fn(value);
    };
    window.addEventListener("focus", () => {
      setTimeout(() => done(resolve, []), 400);
    }, { once: true });

    input.addEventListener("change", async () => {
      const files = [...(input.files || [])];
      if (!files.length) return done(resolve, []);
      const names = [];
      for (const file of files) {
        if (file.size > MAX_ATTACH_BYTES) {
          return done(reject, new Error(
            `${file.name} is ${Math.round(file.size / 1024 / 1024)} MB. ` +
            `Attachments are up to ${MAX_ATTACH_BYTES / 1024 / 1024} MB each.`,
          ));
        }
        try {
          attached.set(file.name, {
            name: file.name,
            type: file.type || "application/octet-stream",
            bytes: await fileToBase64(file),
          });
          names.push(file.name);
        } catch (err) {
          return done(reject, new Error(`${file.name} could not be read.`));
        }
      }
      done(resolve, names);
    });

    input.click();
  });
}

/// A file's bytes as base64, which is what JSON can carry.
function fileToBase64(file) {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onerror = () => reject(reader.error || new Error("read failed"));
    reader.onload = () => {
      // `readAsDataURL` gives "data:<type>;base64,<payload>"; the payload
      // is what the build service wants, so the prefix is dropped here
      // rather than on the wire.
      const out = String(reader.result || "");
      const comma = out.indexOf(",");
      resolve(comma >= 0 ? out.slice(comma + 1) : "");
    };
    reader.readAsDataURL(file);
  });
}

/// The attachments named in a request, in the shape the build service
/// takes. Anything it does not recognise is dropped rather than sent as
/// an empty file.
function attachmentsFor(names) {
  return (names || [])
    .map((name) => attached.get(name))
    .filter(Boolean);
}

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
  // Shape-checked, not just parse-checked.
  //
  // The catch caught bad JSON and nothing else, so anything that PARSED got
  // through: `null` (JSON.parse("null") is null, which defeats the `|| "[]"`
  // fallback), an object, a number, or an array with null members. Each one
  // reached the UI's `[...sessions].sort(...)` and threw
  // "sessions is not iterable" or "Cannot read properties of null" on every
  // paint -- the sidebar died, "My apps" stopped responding, and nothing
  // self-healed it because the bad value stayed in storage across reloads.
  //
  // Proved against the live site by writing each shape and reloading.
  //
  // Anything that is not a usable list is treated as no sessions, and the
  // members are filtered too: one null in the array was enough.
  try {
    const raw = JSON.parse(localStorage.getItem("krate-sessions") || "[]");
    if (!Array.isArray(raw)) return [];
    return raw.filter((s) => s && typeof s === "object" && s.id);
  } catch (e) {
    return [];
  }
}
function saveLocalSessions(list) {
  try { localStorage.setItem("krate-sessions", JSON.stringify(list.slice(0, 60))); } catch (e) {}
}

/* A refusal Studio's UI already knows how to show. The words matter more
 * than the mechanism: they must say what to do next, and never blame the
 * person for standing in a browser. */
/* "85 KB" back into bytes.
 *
 * The build service prettifies the size before sending it, and the Details
 * sheet divides what it is given by 1024. Handing it the string produced
 * "NaN KB". Only KB and MB exist (cloud/builder/src/server.js prettySize),
 * and anything unrecognised answers 0 rather than a guess. */
function bytesOfPretty(size) {
  if (typeof size === "number") return size;
  const m = /^([\d.]+)\s*(KB|MB)$/i.exec(String(size || "").trim());
  if (!m) return 0;
  const n = parseFloat(m[1]);
  if (!isFinite(n)) return 0;
  return Math.round(n * (m[2].toUpperCase() === "MB" ? 1024 * 1024 : 1024));
}

function refuse(message) {
  const err = new Error(message);
  // Flagged so Studio's error wording knows this is a refusal and not a
  // failed build. `plainWords` classifies on provider vocabulary, and a
  // refusal matches none of it, so an unflagged one came out as "The build
  // failed. Press Details for the engine output" -- on sheets that have no
  // Details, about builds that never started.
  err.refusal = true;
  return Promise.reject(err);
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
/* Does this computer already have Krate?
 *
 * A tab cannot find out. There is no way to ask the operating system what
 * opens a .krate, and probing for it would be both unreliable and rude. So
 * the person is asked once and the answer is kept, per browser.
 *
 * Kept in localStorage rather than the account on purpose: this is a fact
 * about the COMPUTER, not about the person. The same account on a work
 * laptop and a home desktop needs two different answers, and an account
 * round-trip would make Run it wait on the network for something the tab
 * can settle instantly.
 */
const HAS_KRATE_KEY = "krate.has.player.v1";
function hasKrateAlready() {
  try {
    return localStorage.getItem(HAS_KRATE_KEY) === "1";
  } catch (e) {
    // Storage blocked. Asking every time is annoying but honest; assuming
    // they have it would send them back to the silent double-click.
    return false;
  }
}
function rememberHasKrate() {
  try { localStorage.setItem(HAS_KRATE_KEY, "1"); } catch (e) {}
}

/* Which computer this is, for the one sentence that differs per system.
 * Same test as krate.tech/open, which is where both branches send people. */
function thisSystem() {
  const ua = navigator.userAgent || "";
  if (/Android|iPhone|iPad|iPod/.test(ua)) return "phone";
  if (/Windows/.test(ua)) return "windows";
  if (/Linux|X11/.test(ua) && !/Mac/.test(ua)) return "linux";
  if (/Mac/.test(ua)) return "mac";
  return "";
}

/* Ask, once, before handing over a file that may not open.
 *
 * The sheet lives in Studio's own markup so it looks like every other
 * sheet; the bridge only fills it in and wires the two answers, because
 * whether a tab can open an app is a browser fact and not Studio's.
 */
function askFirstRun(app) {
  const sheet = document.getElementById("firstRunSheet");
  if (!sheet) {
    // No sheet in this shell. Fall back to the old behaviour rather than
    // swallowing the press.
    downloadApp(app.url, app.name).catch(() => {});
    return;
  }
  const system = thisSystem();
  const sub = document.getElementById("frNeedSub");
  if (sub) {
    sub.textContent = system === "phone"
      ? "Krate runs on computers, not phones. Open this page on your Mac, "
        + "Windows or Linux and your app will open there."
      : "Get it once, the way you got a video player. After that every "
        + "Krate app just opens.";
  }
  const note = document.getElementById("frNote");
  if (note) note.textContent = "";
  sheet.dataset.appUrl = app.url;
  sheet.dataset.appName = app.name || "app.krate";
  sheet.classList.remove("hidden");
}

function currentWebApp(path, version) {
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
  // An older version gets the version in its filename.
  //
  // Downloading v1, v2 and v3 gave three files called "Shopping list.krate",
  // which the browser silently renamed to (1) and (2) -- so a person with
  // three copies in their downloads folder had no way to tell which was
  // which, and the one they double-clicked was a coin toss.
  //
  // Only when a version is named AND it is not the one the page is showing:
  // the common case is one app with one name, and putting "v3" on that
  // would be noise.
  const base = name || "app.krate";
  if (version && version > 1) {
    const dot = base.lastIndexOf(".krate");
    const stem = dot > 0 ? base.slice(0, dot) : base;
    return { url, name: `${stem} v${version}.krate` };
  }
  return { url, name: base };
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
/* The build this tab is watching, written down.
 *
 * `bridge.job` is memory, so a refresh lost it -- and with it the only
 * handle on a build that was still running on the service. The person came
 * back to Home with an empty thread, no "making now" bar, and no way to
 * reach the app they had waited for. It was still being built and still
 * counted against their allowance; they simply could not see it.
 *
 * Stored per session so reopening the right one picks it back up, and
 * cleared the moment the job settles.
 */
const RUNNING_KEY = "krate.web.running.v1";
function rememberRunningJob(jobId, sessionId, request) {
  try {
    localStorage.setItem(RUNNING_KEY, JSON.stringify({
      job: jobId, session: sessionId || "", request: request || "", at: Date.now(),
    }));
  } catch (e) {}
}
function forgetRunningJob() {
  try { localStorage.removeItem(RUNNING_KEY); } catch (e) {}
}
function runningJob() {
  try {
    const raw = localStorage.getItem(RUNNING_KEY);
    if (!raw) return null;
    const rec = JSON.parse(raw);
    // The job id must be a STRING. An object here became
    // `/build/[object Object]` in a real request; the builder answers 404,
    // so it cost a wasted round trip on every page load rather than
    // anything worse, but a request built from a shape nobody checked is
    // not a request worth sending.
    if (!rec || typeof rec.job !== "string" || !rec.job) return null;
    // A build that cannot still be running is not worth reattaching to. The
    // service drops a job long before this; the point is only to stop an
    // ancient record making the page chase something that is gone.
    //
    // `at` is validated as a NUMBER first, because Number("yesterday") is
    // NaN and every NaN comparison is false -- so a non-numeric timestamp
    // made the record immortal and the page chased it forever, on every
    // load, with no way to clear it.
    const at = Number(rec.at);
    if (!Number.isFinite(at) || at <= 0) {
      forgetRunningJob();
      return null;
    }
    if (Date.now() - at > 60 * 60 * 1000) {
      forgetRunningJob();
      return null;
    }
    return rec;
  } catch (e) {
    return null;
  }
}

function watchJob(jobId, request, sessionId) {
  bridge.job = jobId;
  bridge.jobResult = null;
  rememberRunningJob(jobId, sessionId, request);
  let lastLine = "";
  let lastShot = "";
  return new Promise((resolve, reject) => {
    const tick = async () => {
      let job;
      try {
        job = await builder(`/build/${jobId}`);
      } catch (err) {
        clearInterval(bridge.poll);
        forgetRunningJob();
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
        forgetRunningJob();
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
        forgetRunningJob();
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
      where_kept: k.set ? `saved to your account${k.tail ? `, ends ${k.tail}` : ""}` : "",
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

  async create_app({ request, session, starterShape, attachments } = {}) {
    // Plan mode means plan mode. Studio's chat has an escape hatch -- typing
    // "build it" during planning starts the build -- and on a desktop that
    // is right, because the person chose to plan in that moment. Here they
    // chose a MODE, before typing, and a mode that sometimes builds is not
    // a mode. The refusal says how to change it.
    if (webMode() === "plan") {
      // Flagged so Studio offers the switch as a button rather than telling
      // them where to find it. Plan is remembered across visits, so somebody
      // who tried it once meets this on every build afterwards -- and
      // "switch to Build in the box below" is a instruction to go and do the
      // thing they thought they had just done.
      const err = new Error(
        "You are in Plan mode, so nothing was built. Switch to Build and I will make this app.",
      );
      err.refusal = true;
      err.planMode = true;
      return Promise.reject(err);
    }
    const overCreate = tooLong(request, "request");
    if (overCreate) return overCreate;
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
        body: JSON.stringify({
          request,
          device: deviceId(),
          shape: starterShape || "",
          // The bytes, not the names: the build service writes them back
          // to disk and hands the engine `--attach <file>` for each.
          attachments: attachmentsFor(attachments),
        }),
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
  async revise_app({ path, change, attachments } = {}) {
    if (!bridge.token) return refuse("Sign in to change your app.");
    const overChange = tooLong(change, "change");
    if (overChange) return overChange;
    const id = jobIdOf(path);
    if (!id) return refuse("This app was not made here, so it cannot be changed here. Open it in Studio on your computer.");
    let started;
    try {
      started = await builder(`/build/${id}/revise`, {
        method: "POST",
        body: JSON.stringify({ change, device: deviceId(), attachments: attachmentsFor(attachments) }),
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
   * asks on a desktop. Attachments go with it, because `krate plan` takes
   * them: the questions can then be about the file rather than only the
   * sentence. */
  async plan_request({ request, attachments } = {}) {
    if (!bridge.token) return refuse("Sign in first.");
    const overPlan = tooLong(request, "request");
    if (overPlan) return overPlan;
    const answer = await builder("/plan", {
      method: "POST",
      body: JSON.stringify({
        request,
        device: deviceId(),
        attachments: attachmentsFor(attachments),
      }),
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
    // Somebody who stopped a build must not be offered it again on their
    // next visit. Every other exit from the poll forgets it; this one was
    // missed, and the test that counts them is what found it.
    forgetRunningJob();
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
  /* The list this tab already holds, with no network at all.
   *
   * Studio paints the sidebar from `sessions_list`, and on a desktop that
   * reads local disk in milliseconds. In a tab it was a hub round trip
   * that the paint waited on, so pressing Back sat on the old screen for
   * seconds before Home appeared -- with the answer already in local
   * storage the whole time. Home now paints from this and refreshes from
   * the hub behind it. */
  async sessions_local() {
    return localSessions();
  },

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
   * opens on the person's own computer.
   *
   * It used to download and then say "Double-click the file". For somebody
   * who has never installed Krate, double-clicking does NOTHING -- no
   * handler, no window, no error saying why. That is the first thing a
   * person does after waiting minutes for their app, and it dead-ended in
   * silence.
   *
   * A tab cannot tell whether this computer has Krate, so it does not
   * guess. It asks once, remembers the answer, and after that Run it goes
   * straight to the download. */
  async open_app({ path, version } = {}) {
    const app = currentWebApp(path, version);
    if (!app) return refuse("A browser cannot open the app itself. Download the file. It opens on your Mac, Windows or Linux.");
    if (!hasKrateAlready()) {
      askFirstRun(app);
      // Not a refusal: the sheet is now asking, and a red line under the
      // share row while a dialog is open reads as two things going wrong.
      return "asking";
    }
    await downloadApp(app.url, app.name);
    return refuse("Downloaded. Double-click the file on your Mac, Windows or Linux. That is where the app really runs.");
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
  /* Attaching a file, in a tab.
   *
   * A desktop picker answers with PATHS, and everything downstream --
   * the chips, the engine's `--attach` -- is written against a path. A
   * browser has no paths: it has a File the person chose, and bytes we
   * must carry ourselves.
   *
   * So the browser keeps both. `bridge.attached` maps a name to its
   * bytes, and the name alone is what Studio's own UI sees, so the chips
   * and the remove button work unchanged. The bytes ride with the build
   * request and are written back to disk on the build service, which is
   * where `--attach` can reach them.
   */
  pick_files() {
    return pickLocalFiles({ accept: "", multiple: true });
  },
  pick_image() {
    return pickLocalFiles({ accept: "image/*", multiple: false });
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
    return refuse("The card is made in Studio on your computer. Download the file and open it there. Send a link works from here.");
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
      return refuse(`Downloaded the project: ${n} file${n === 1 ? "" : "s"}. Open the folder with cargo, or in Studio on your computer.`);
    }
    const app = currentWebApp(path);
    if (!app) return refuse("Check your downloads folder.");
    await downloadApp(app.url, app.name);
    return refuse("Downloaded. Check your downloads folder.");
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

  /* ---- the gallery ------------------------------------------------------
   *
   * Apps other people published. This is a hub listing and needs nothing
   * from a desktop, but the bridge had no answer for it -- so the Shared
   * screen showed "That part of Studio needs the app on your computer"
   * over a feature that works perfectly in a tab.
   *
   * Studio hands the JSON straight to its own renderer, so this returns
   * the hub's body as text exactly as the desktop command does.
   */
  async cloud_apps({ q, cat } = {}) {
    const params = new URLSearchParams();
    if (q) params.set("q", q);
    if (cat) params.set("cat", cat);
    const query = params.toString();
    const out = await hub(`/apps${query ? `?${query}` : ""}`);
    return typeof out === "string" ? out : JSON.stringify(out);
  },

  /* Opening one: a browser cannot run a .krate, so this hands over the
   * app's page, where it can be downloaded and read about. */
  async cloud_run({ url } = {}) {
    if (!url) return refuse("That app has no link yet.");
    window.open(url, "_blank", "noopener");
  },

  /* The Details sheet on a finished app.
   *
   * A desktop reads this out of the .krate on disk. A browser has no disk,
   * but it does not need one: the build service already reported what the
   * app asks for and how big it is, and that is everything this sheet
   * shows. Without an answer the sheet's headline trust line read
   * `Error: That part of Studio needs the app on your computer.`
   *
   * Two shape conversions, because the build service and this sheet do not
   * speak the same dialect:
   *   - `asks` arrives as capability strings and the sheet wants
   *     {cap, words} rows.
   *   - `size` arrives ALREADY pretty ("85 KB") and the sheet divides it by
   *     1024, which on a string is NaN. So it is parsed back to bytes.
   */
  async app_info({ path } = {}) {
    const words = (cap) =>
      typeof window.friendlyAsk === "function" ? window.friendlyAsk(cap) || cap : cap;
    const shape = (caps, size) => ({
      size,
      asks: (caps || []).map((cap) => ({ cap, words: words(cap) })),
      capabilities: caps || [],
    });

    // An app somebody else published, opened from the gallery. It was
    // never built in this tab, so there is nothing in `jobResult` to read
    // -- and the permission list is the whole point of that page, because
    // it is what a stranger decides on before downloading anything. The
    // hub records it at publish time and serves it from /meta/<id>.
    //
    // Without this the detail page said "Could not read this app right
    // now" under the heading "WHAT IT IS ALLOWED TO DO", which is the
    // least reassuring possible answer to "should I trust this".
    const id = String(path || "").match(/\/a\/([A-Za-z0-9_-]+)/);
    if (id) {
      try {
        const out = await hub(`/meta/${id[1]}`);
        const m = (out && out.meta) || {};
        // A null list means "published before we recorded them", which is
        // not the same as "asks for nothing". Refusing here lets the page
        // say so rather than claim an empty list.
        if (!Array.isArray(m.capabilities)) {
          return refuse("This app was published before Krate recorded what apps ask for.");
        }
        return shape(m.capabilities, Number(m.size) || 0);
      } catch (err) {
        if (err && err.refusal) throw err;
        return refuse("Could not read what this app asks for just now.");
      }
    }

    const r = bridge.jobResult;
    if (r) return shape(r.asks || [], bytesOfPretty(r.size));

    // `jobResult` is this TAB's memory of the build it ran, so a reload
    // empties it -- and Details then said "There is no app to read yet" on
    // a finished app that was still on screen, with its permissions listed
    // right there on the done card. The sessions carry the same `asks` and
    // `size`, saved when the build finished, so an app that is showing is
    // never refused as "not made here" (the K-366 cure, applied to the one
    // path that still had not learned it).
    const withAsks = localSessions().filter(
      (s) => s && s.result && Array.isArray(s.result.asks),
    );
    // Matched on the path the caller passed, not on whichever session is
    // newest: somebody reading the details of an app they made last week
    // must not be shown this morning's permissions. Newest is the fallback
    // for the caller that passes nothing, which is the done card asking
    // about the app it is already showing.
    const wanted = String(path || "");
    const saved =
      (wanted && withAsks.find((s) => String(s.result.path || "") === wanted)) ||
      (!wanted && withAsks.sort((a, b) => (b.updated || 0) - (a.updated || 0))[0]);
    if (saved) return shape(saved.result.asks, bytesOfPretty(saved.result.size));
    return refuse("There is no app to read yet.");
  },

  /* The failed request, sent to us so the tool improves.
   *
   * A plain hub POST with nothing desktop about it, and the field names
   * already match, so this passes straight through. Unbridged, the
   * "Send the request" button on the failure card wrote
   * `Error: That part of Studio needs the app on your computer.` into the
   * sheet -- on the one screen somebody reaches only after a build has
   * already let them down.
   */
  async make_for_me({ email, request, answers, agent, why } = {}) {
    return hub("/makeit", {
      method: "POST",
      body: JSON.stringify({
        email: email || "",
        request: request || "",
        answers: answers || "",
        agent: agent || "",
        why: why || "",
      }),
    });
  },

  /* ---- support: the same desk, from a tab -------------------------------
   *
   * Three hub endpoints that need nothing from a desktop, and had no
   * browser answer -- so "Contact support" in a tab wrote
   * `Error: That part of Studio needs the app on your computer.` into the
   * sheet, which is the worst possible reply to somebody who came to that
   * sheet because something was already wrong.
   *
   * The desktop renames one field on its way out (`message` -> `text`);
   * that rename lives here too, because the hub's contract is the hub's.
   */
  async support_new({ subject, message, email } = {}) {
    return hub("/support/new", {
      method: "POST",
      body: JSON.stringify({ subject: subject || "", text: message || "", email: email || "" }),
    });
  },
  async support_list({ keys } = {}) {
    return hub("/support/list", {
      method: "POST",
      body: JSON.stringify({ keys: keys || [] }),
    });
  },
  async support_reply({ id, key, message } = {}) {
    return hub("/support/reply", {
      method: "POST",
      body: JSON.stringify({ id, key: key || "", text: message || "" }),
    });
  },
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
      <span class="who">${who}${model ? `, ${model}` : ""}${r.rounds ? `, ${r.rounds} rounds` : ""}</span>
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
/* A thumb needs 44. Set here rather than in style.css because this block
   is injected into the head at runtime and so wins on source order --
   a rule in the stylesheet for these buttons would be silently overridden.
   Build and Plan are the choice the composer is built around, so they are
   not the place to save eight pixels. */
@media (pointer: coarse) {
  .web-mode button { min-height: 44px; padding: 8px 16px; }
  .web-mode-hint { font-size: 11.5px; }
}
`;

function paintMode() {
  const mode = webMode();
  document.querySelectorAll(".web-mode button").forEach((b) => {
    b.setAttribute("aria-pressed", String(b.dataset.mode === mode));
  });
  const box = document.getElementById("homePrompt");
  if (box) {
    box.placeholder = mode === "plan"
      ? "Describe an app. You will get a plan, not a build…"
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
    <button type="button" data-mode="plan" title="Get a plan only, nothing is built">Plan</button>`;
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
  // Refreshing the AI list asks which coding tools are installed on this
  // machine. In a tab the answer is always the same one AI, so the button
  // is a control that cannot change anything.
  const aiRefresh = document.getElementById("aiRefresh");
  if (aiRefresh) aiRefresh.closest(".ai-actions")?.remove();
  // A settings group whose every row has gone should go too, or the sheet
  // grows headings standing over nothing.
  document.querySelectorAll(".set-group, .set-panel").forEach((group) => {
    if (!group.querySelector(".set-row")) group.remove();
  });
}

/* Wording that is true on a desktop and false in a tab.
 *
 * Studio says "on this computer" because that is where a desktop Studio
 * keeps things. In a browser the apps live in the account, reachable from
 * any machine the person signs in on -- which is the whole reason the web
 * Studio exists. Leaving the desktop sentence in place tells somebody
 * their work is stuck on the device they happen to be holding.
 *
 * Rewritten rather than removed: each of these sentences is doing a job,
 * and a blank where an explanation was is its own defect. Matched on the
 * exact desktop string so a future edit to the copy fails loudly here
 * (the sentence simply stays) instead of silently rewriting something
 * else. */
const WEB_WORDING = [
  [
    "Everything you have made on this computer. Open one to change it or send it on.",
    "Everything you have made. Open one to change it or send it on.",
  ],
  ["all together, on this computer", "all together"],
  ["Your apps stay on this computer", "Your apps stay in your account"],
  ["How Krate looks on this computer.", "How Krate looks for you."],
  // The session log is the local build workspace: the agent's transcript,
  // the generated lib.rs, the Cargo.toml. A browser build happens on the
  // build service and leaves none of that here, so the desktop sentence
  // promises an attachment that cannot be made. The ticket itself works;
  // only the promise about what rides with it is wrong.
  [
    "Goes to us with the session log attached; replies land in this window",
    "Goes to us, and replies land in this window",
  ],
  // The AI panel, which on a desktop lists the coding tools the person has
  // installed and how to fix each one. In a tab there is one AI, ours, and
  // nothing to install, pick, refresh or sign into -- so every sentence in
  // this panel was false at once: "Installed tools" over a list of one,
  // "the AI you already have" when it is ours, a Refresh button for a list
  // that cannot change, and a pointer to a Terminal fold a browser has no
  // way to open.
  ["Installed tools", "The AI that builds your app"],
  // The two long sentences in this panel wrap across lines in the HTML, so
  // their text nodes carry the source's newlines and indentation. This
  // table matches whole text nodes exactly, which a wrapped sentence
  // cannot survive -- they are rewritten in `speakWebAi` instead, on
  // collapsed whitespace.
];

/* The composer's hints are written for a keyboard and a wide screen. On a
 * touch screen the first is untrue -- there is no Return key to press --
 * and the second is 74 characters, which wrapped to two right-aligned
 * lines and left the word "edits" sitting alone against the edge.
 *
 * Applied only under `pointer: coarse`, so a laptop keeps the longer
 * sentence, and re-applied on a timer because Studio rewrites this line
 * itself when a build finishes (setRevisePlaceholders). */
const TOUCH_WORDING = [
  ["↩ to make it · shift-↩ for a new line", "Describe it in a sentence"],
  [
    "changes edit the app in place · a few minutes, the AI reads before it edits",
    "Changes edit the app in place · a few minutes",
  ],
];

/* The placeholders have the same problem for the same reason. Measured in
 * the phone composer: the field is 249px wide and "Want it different? Say
 * what to change…" needs 252, so it was cut mid-sentence -- the screenshot
 * showed "Say what to" and nothing after it. */
const TOUCH_PLACEHOLDERS = [
  ["Want it different? Say what to change…", "What should change?"],
  ["Describe the app you want…", "Describe your app…"],
];

function speakTouchWording() {
  // Width as well as touch. A touchscreen laptop reports `pointer: coarse`
  // at 1440px, where the composer has all the room the long sentence needs
  // -- shortening it there would be a downgrade for no reason. The
  // breakpoint matches the stylesheet's own phone rules.
  if (!window.matchMedia) return;
  if (!matchMedia("(pointer: coarse)").matches) return;
  if (!matchMedia("(max-width: 860px)").matches) return;
  const hint = document.getElementById("composerHint");
  if (hint) {
    const text = hint.textContent.trim();
    for (const [wide, touch] of TOUCH_WORDING) {
      if (text === wide) { hint.textContent = touch; break; }
    }
  }
  const box = document.getElementById("prompt");
  if (box) {
    for (const [wide, touch] of TOUCH_PLACEHOLDERS) {
      if (box.placeholder === wide) { box.placeholder = touch; break; }
    }
  }
}

/* The Details sheet's source row, which no static rewrite can reach.
 *
 * `session_source_dir` answers "source" -- a sentinel `reveal` matches on
 * to hand over the Cargo project as a download. Studio prints whatever it
 * is given as a file path, so the sheet showed the bare word "source"
 * where a path belongs, over buttons reading "Open the folder" and "Copy
 * the path". Both work -- they download the project -- but neither says
 * so, and the path they name does not exist.
 *
 * Written after the sheet is filled rather than rewritten from the table
 * above, because `showInfo` sets these nodes itself when the sheet opens:
 * a boot-time pass would be overwritten by the next click.
 */
function speakWebSource() {
  const line = document.getElementById("infoSourcePath");
  if (line && line.textContent.trim() === "source") {
    line.textContent = "The app's Rust project, ready to download.";
  }
  const open = document.getElementById("infoOpenSource");
  if (open && open.textContent.trim() === "Open the folder") {
    open.textContent = "Download the project";
  }
  // There is no path to put on a clipboard in a tab.
  const copy = document.getElementById("infoCopySource");
  if (copy) copy.remove();
  // The terminal line names the AI that made the app, and on the web that
  // is ours -- but `--agent krate` is not a thing the CLI accepts, so
  // copying the line gave somebody a command that cannot run. On their own
  // machine the AI is theirs, and the placeholder says so.
  const cmd = document.getElementById("infoCmd");
  if (cmd && cmd.textContent.includes("--agent krate")) {
    cmd.textContent = cmd.textContent.replace("--agent krate", "--agent <your-ai>");
  }
}

/* The AI panel's two wrapped sentences.
 *
 * On a desktop this panel lists the coding tools the person has installed
 * and the one-line fix for each. In a tab there is one AI, ours, and
 * nothing to install, pick or sign into -- so both sentences were false,
 * and one of them pointed at a Terminal fold a browser cannot open.
 *
 * Matched on collapsed whitespace rather than through WEB_WORDING, because
 * both wrap across several lines in the HTML: their text nodes carry the
 * source's newlines and indentation, and the table compares whole nodes.
 */
const WEB_AI_WORDING = [
  [
    /^Krate works with the AI you already have\. Pick one\s+that is ready, or follow its one-line fix\. Its sign-in stays with\s+that tool, and Krate never holds its keys\.$/,
    "Krate's own AI builds your app here. Nothing to install, and no key of yours is ever held.",
  ],
  [
    /^Signed in somewhere else just now\? Press Refresh\.\s+Prefer a terminal\? Each fix's exact command is under its Terminal fold\.$/,
    "On your own machine, Krate Studio drives the AI you already have instead.",
  ],
];

function speakWebAi() {
  for (const el of document.querySelectorAll(".set-sub, .ai-note")) {
    const text = el.textContent.trim();
    for (const [desktop, web] of WEB_AI_WORDING) {
      if (desktop.test(text)) { el.textContent = web; break; }
    }
  }
}

function speakWebWording() {
  // Text nodes only: rewriting innerHTML would drop the handlers Studio
  // has already bound to the buttons inside these blocks.
  const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
  const hits = [];
  for (let n = walker.nextNode(); n; n = walker.nextNode()) {
    const text = n.nodeValue.trim();
    if (!text) continue;
    for (const [desktop, web] of WEB_WORDING) {
      if (text === desktop) hits.push([n, n.nodeValue.replace(desktop, web)]);
    }
  }
  for (const [node, value] of hits) node.nodeValue = value;
  return hits.length;
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
  speakWebWording();
  speakTouchWording();
  // Studio rewrites the composer's hint itself when a build finishes, so
  // a one-shot pass at boot would be undone a minute later. Watching the
  // one element is cheaper and more honest than re-scanning on a timer.
  const hintEl = document.getElementById("composerHint");
  const promptEl = document.getElementById("prompt");
  if (window.MutationObserver) {
    const watch = new MutationObserver(speakTouchWording);
    if (hintEl) watch.observe(hintEl, { childList: true, characterData: true, subtree: true });
    // The placeholder is an attribute, not a text node, so it needs its
    // own filter -- the hint's childList watch would never see it change.
    if (promptEl) watch.observe(promptEl, { attributes: true, attributeFilter: ["placeholder"] });
  }
  // Crossing the breakpoint the other way needs Studio's own longer text
  // back, which only it knows -- so ask for it rather than keeping a copy:
  // the screen that owns this line repaints it.
  if (window.matchMedia) {
    matchMedia("(max-width: 860px)").addEventListener?.("change", (e) => {
      if (!e.matches && typeof window.setRevisePlaceholders === "function") {
        try { window.setRevisePlaceholders(); } catch (err) {}
      }
      speakTouchWording();
    });
  }
  // The settings sheet is in the page from the start, but a pane can be
  // painted later; trim again when one is opened.
  document.addEventListener("click", () => setTimeout(() => {
    trimDesktopOnly();
    speakWebWording();
    speakWebSource();
    speakWebAi();
  }, 50), true);
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

/* Two tabs, one account.
 *
 * People leave tabs open. Somebody builds in one and switches back to
 * the other, and that tab was showing the world as it was before: no new
 * app in the list, and a composer still offering to make the first one.
 * With one free app that reads as the app having vanished.
 *
 * The money was never at risk -- the wall is counted on the hub against
 * the account and the device, so a second tab cannot get a second free
 * app. This is only the display, and `storage` fires in every OTHER tab
 * whenever one of them writes, so the fix is to repaint on it.
 *
 * Guarded against a repaint storm: a burst of writes during one build
 * settles into a single repaint.
 */
(function keepTabsInStep() {
  if (!window.addEventListener) return;
  let due = null;
  window.addEventListener("storage", (event) => {
    // Sessions AND the make counter.
    //
    // This watched "krate-sessions" alone, so the other tab repainted its
    // app list and kept a stale free-app count beside it -- which is the
    // second half of the very confusion the comment above describes: the
    // composer still offering a free app that was already used. Both keys
    // are written when a build finishes, and both are display.
    if (event.key !== "krate-sessions" && event.key !== "krateMakes") return;
    clearTimeout(due);
    due = setTimeout(() => {
      // Studio owns its own painting; ask it to redraw rather than
      // reaching into its DOM from here.
      if (typeof window.renderSessions === "function") {
        try { window.renderSessions(localSessions()); } catch (e) {}
      }
      if (typeof window.renderShelf === "function") {
        try { window.renderShelf(); } catch (e) {}
      }
      // The counter has its own painter. Without this the chip in the
      // other tab keeps the number it had when that tab was opened.
      if (typeof window.renderFreeCount === "function") {
        try { window.renderFreeCount(); } catch (e) {}
      }
    }, 250);
  });
})();

console.info("krate: studio bridge ready (hub + builder)");

/* The browser's own back button, and the phone's back swipe.
 *
 * Studio swaps screens by class: showView hides one and shows another, and
 * nothing was ever written to the browser's history. So back from a session
 * did not go up one screen -- it LEFT Studio, because the previous history
 * entry was whatever page the person was on before they arrived. On a phone
 * that is the edge swipe, which people do constantly and without thinking.
 *
 * Fixed here rather than in app.js because it is a browser fact: the desktop
 * shell has no browser back button and no history to keep in step.
 *
 * The screen is kept in history.state rather than the URL. A ?view= would be
 * shareable, which sounds better until somebody sends a friend a link to a
 * session that only exists in their browser.
 */
(function keepBrowserBackHonest() {
  if (!window.history || !window.history.pushState) return;
  if (typeof window.showView !== "function") {
    // app.js has not booted yet. It declares showView at the top level of a
    // classic script, so it lands on window -- wait for it rather than
    // guessing at load order.
    let tries = 0;
    const wait = setInterval(() => {
      if (typeof window.showView === "function" || ++tries > 200) {
        clearInterval(wait);
        if (typeof window.showView === "function") keepBrowserBackHonest();
      }
    }, 50);
    return;
  }

  const real = window.showView;
  // Where back goes from each screen. A screen missing from this map is a
  // screen back should leave alone.
  const UP = {
    session: "home",
    apps: "home",
    cloud: "home",
    appDetail: "cloud",
  };
  // The gate and onboarding are not places to go back INTO: a person who has
  // signed in should not land on the sign-in screen by pressing back.
  const NO_ENTRY = new Set(["gate", "onboard"]);

  let current = null;
  let restoring = false;

  window.showView = function (name) {
    real(name);
    if (restoring) return;
    if (name === current) return;
    current = name;
    try {
      if (NO_ENTRY.has(name)) {
        history.replaceState({ krateView: name }, "");
      } else {
        history.pushState({ krateView: name }, "");
      }
    } catch (e) {
      // A history that refuses to be written is not worth breaking the
      // screen change over.
    }
  };

  window.addEventListener("popstate", (event) => {
    const want = event.state && event.state.krateView;
    if (!want) {
      // Popped past Studio's own entries: send them to the screen back
      // would have gone to, rather than letting the page sit on whatever
      // was showing. Leaving Studio needs one more press than that.
      const up = UP[current];
      if (!up) return;
      restoring = true;
      try { real(up); current = up; } finally { restoring = false; }
      try { history.replaceState({ krateView: up }, ""); } catch (e) {}
      return;
    }
    restoring = true;
    try { real(want); current = want; } finally { restoring = false; }
  });

  // Whatever is showing when this runs is the first entry, so the first
  // back press has somewhere to land.
  try {
    const showing = ["home", "session", "apps", "cloud", "appDetail", "gate", "onboard"]
      .find((n) => {
        const el = document.getElementById({
          home: "viewHome", session: "viewSession", apps: "viewApps",
          cloud: "viewCloud", appDetail: "viewApp", gate: "viewGate",
          onboard: "viewOnboard",
        }[n]);
        return el && !el.classList.contains("hidden");
      });
    if (showing) {
      current = showing;
      history.replaceState({ krateView: showing }, "");
    }
  } catch (e) {}
})();

/* The two answers to "do you have Krate?".
 *
 * Wired here rather than in app.js because the question only exists in a
 * browser: the desktop Studio opens the app itself and never asks.
 */
(function wireFirstRun() {
  const sheet = document.getElementById("firstRunSheet");
  if (!sheet) return;
  const note = document.getElementById("frNote");
  const app = () => ({
    url: sheet.dataset.appUrl || "",
    name: sheet.dataset.appName || "app.krate",
  });

  document.getElementById("frHaveBtn")?.addEventListener("click", async () => {
    // Taking them at their word is the whole point of asking. If they were
    // wrong, the download still sits in their downloads folder and the
    // Ship it sheet still offers the gift, so nothing is lost.
    rememberHasKrate();
    sheet.classList.add("hidden");
    const { url, name } = app();
    try {
      await downloadApp(url, name);
    } catch (err) {
      if (note) note.textContent = String((err && err.message) || err);
      sheet.classList.remove("hidden");
    }
  });

  document.getElementById("frNeedBtn")?.addEventListener("click", async () => {
    const { url, name } = app();
    // Their app first, then the player. In this order the file is already
    // waiting when the install finishes, so the last thing they do is open
    // their own app rather than hunt for it.
    //
    // A phone gets no download at all: nothing there can open it, and a
    // file that cannot open is worse than a sentence saying so.
    if (thisSystem() !== "phone") {
      try {
        await downloadApp(url, name);
      } catch (e) {
        // The player page is still worth reaching even if this failed.
      }
    }
    sheet.classList.add("hidden");
    window.open("https://krate.tech/open/", "_blank", "noopener");
  });
})();

/* "For someone new to Krate", in a tab.
 *
 * That option makes a gift: one double-clickable file that installs the
 * player and then opens the app. It is made by the krate binary on the
 * sender's own machine, so a browser cannot make one -- and the sheet said
 * so only AFTER the person had picked the option, picked an operating
 * system, and waited. Three clicks to reach "made in Studio on your
 * computer", on a screen where nothing had hinted at it.
 *
 * The link beside it already does this job here: anyone who opens a shared
 * link gets the app, and gets Krate once if they do not have it. So the
 * option says that instead of pretending, and the OS buttons that lead
 * nowhere are not shown at all.
 *
 * Rewritten from the bridge rather than changed in Studio's markup because
 * it is only true in a browser: the desktop makes the gift perfectly well.
 */
(function tellTheTruthAboutTheGift() {
  const btn = document.getElementById("sendWrapBtn");
  if (!btn) return;
  const line = btn.querySelector("span");
  if (line) {
    line.textContent =
      "Making that file needs Krate on your own computer. From here, "
      + "share a link instead: whoever opens it gets the app, and gets "
      + "Krate once if they do not have it.";
  }
  // It is not a button here, so it must not look like one. Left in place
  // rather than removed because it answers a real question -- what happens
  // to a friend who has never heard of Krate -- and the answer is the line
  // above it.
  btn.disabled = true;
  btn.style.cursor = "default";
  btn.style.opacity = "0.72";
  const os = document.getElementById("sendWrapOs");
  if (os) os.remove();
})();

/* Pick a build back up after a refresh.
 *
 * The job id is now written down when a build starts, so a tab that comes
 * back can reattach to a build still running on the service. Without this
 * the person landed on Home with an empty thread and no way to reach the
 * app they had waited minutes for -- while it went on being built, and went
 * on counting against their allowance.
 *
 * Only reattaches when the job is genuinely still going. A job that
 * finished while the tab was away is left alone: its result was already
 * written into the session when it settled, so the session shows the app
 * the ordinary way.
 */
(function pickTheBuildBackUp() {
  const rec = runningJob();
  if (!rec) return;
  let tries = 0;
  const wait = setInterval(async () => {
    if (++tries > 100) { clearInterval(wait); return; }
    if (typeof window.openSession !== "function") return;
    clearInterval(wait);
    let job;
    try {
      job = await builder(`/build/${rec.job}`);
    } catch (e) {
      // The service does not know it any more. Nothing to reattach to.
      forgetRunningJob();
      return;
    }
    if (!job || job.state === "done" || job.state === "error") {
      forgetRunningJob();
      return;
    }
    const session = localSessions().find((s) => s && s.id === rec.session);
    if (session && typeof window.openSession === "function") {
      try { window.openSession(session); } catch (e) {}
    }
    // Re-enter the same polling loop the build started with, so the card
    // moves again from wherever the build has got to, and hand the promise
    // to Studio so it puts the build card back rather than sitting on
    // whatever stage the restored session happened to show.
    const again = watchJob(rec.job, rec.request, rec.session);
    if (typeof window.resumeRunningBuild === "function") {
      window.resumeRunningBuild(rec.request, again);
    } else {
      again.catch(() => {});
    }
  }, 100);
})();
