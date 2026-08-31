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
 * is a far bigger thing than counting three free apps and not a trade we
 * are willing to make.
 *
 * So this is a plain random id, stored once. A determined person can clear
 * it, and that is accepted. What it stops is the ordinary case -- a second
 * email address for three more apps -- because the id survives signing out
 * and signing in as someone else. The account key catches the rest, and
 * the real backstop is that abuse costs more effort than $12.
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

  async create_app({ request } = {}) {
    // Nobody makes an app without an account: it is how the three free
    // ones are counted, and how the work belongs to someone. But the
    // sentence they just typed must survive the round trip -- being asked
    // to remember and retype it is the moment a person decides the
    // product is careless.
    if (!bridge.token) {
      try { localStorage.setItem(PENDING_KEY, request || ""); } catch (e) {}
      await COMMANDS.login_browser();
      return new Promise(() => {});  // the page is navigating away
    }
    const started = await builder("/build", {
      method: "POST",
      body: JSON.stringify({ request, device: deviceId() }),
    });
    bridge.job = started.id;
    bridge.jobResult = null;

    return new Promise((resolve, reject) => {
      const tick = async () => {
        let job;
        try {
          job = await builder(`/build/${bridge.job}`);
        } catch (err) {
          clearInterval(bridge.poll);
          return reject(err);
        }

        // Studio's UI drives its stages by reading the engine's lines, so
        // feeding it those lines makes the same card move the same way.
        if (job.line && window.onBuildLine) window.onBuildLine(job.line);
        if (job.shot && window.onBuildShot) window.onBuildShot(job.shot);

        if (job.state === "done") {
          clearInterval(bridge.poll);
          bridge.jobResult = job.result;
          return resolve({
            path: `${BUILDER}${job.result.download}`,
            name: `${job.result.name}.krate`,
            size: job.result.size,
            asks: job.result.asks || [],
            shot: job.result.shot || "",
          });
        }
        if (job.state === "failed" || job.state === "stopped") {
          clearInterval(bridge.poll);
          return reject(new Error(job.error || "that build stopped"));
        }
      };
      bridge.poll = setInterval(tick, 1500);
      tick();
    });
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

  async sessions_list() {
    return localSessions();
  },

  async session_save({ session } = {}) {
    const list = localSessions().filter((s) => s.id !== session.id);
    list.unshift(session);
    saveLocalSessions(list);
  },

  async session_delete({ id } = {}) {
    saveLocalSessions(localSessions().filter((s) => s.id !== id));
  },

  async session_shot({ id } = {}) {
    const found = localSessions().find((s) => s.id === id);
    return (found && found.result && found.result.shot) || "";
  },

  /* ---- sharing ---------------------------------------------------------- */

  async publish(args = {}) {
    return hub("/publish", { method: "POST", body: JSON.stringify(args) });
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

  open_app() {
    return refuse("A browser cannot open the app itself. Download the file -- it opens on your Mac, Windows or Linux.");
  },
  open_krate() {
    return refuse("Download the file and double-click it. That is where an app really runs.");
  },
  autorun() {
    return refuse("Download the file to run it.");
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
  reveal() {
    return refuse("Check your downloads folder.");
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
