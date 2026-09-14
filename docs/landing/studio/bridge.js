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
  const url = String(path || (bridge.jobResult && `${BUILDER}${bridge.jobResult.download}`) || "");
  if (!/^https?:/.test(url)) return null;
  const name = (bridge.jobResult && bridge.jobResult.name) ? `${bridge.jobResult.name}.krate` : "app.krate";
  return { url, name };
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

  async create_app({ request, session } = {}) {
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
        body: JSON.stringify({ request, device: deviceId() }),
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
  async session_source_dir() {
    return refuse("The source travels inside the .krate; download it and open it in Studio to edit the files by hand.");
  },
  async reveal({ path } = {}) {
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

function speakWeb() {
  const swaps = [
    [".ob-p", "Krate hands your words to our AI. Nothing to install."],
  ];
  for (const [selector, words] of swaps) {
    const el = document.querySelector(selector);
    if (el) el.textContent = words;
  }
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

console.info("krate: studio bridge ready (hub + builder)");
