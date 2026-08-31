/* Make an app, in the browser.
 *
 * Studio's flow, in a tab: one box, the same four build stages, the same
 * forming card, the same done screen. What differs is only WHERE the work
 * happens -- our machine instead of theirs -- so the page reads progress
 * from the hub rather than from a local process.
 *
 * No Krate app runs here. This page makes a file and hands it over; the
 * app itself runs on the desktop player, which is where an app belongs.
 */

const HUB = "https://hub.krate.tech";
const $ = (id) => document.getElementById(id);

const state = {
  token: null,       // krs_* session token, the same door Studio uses
  me: null,          // { user, plan, referral } from /me
  job: null,         // the build in flight
  result: null,      // the finished app
  request: "",       // what they asked for, kept for "change it"
  stage: -1,
};

/* The same four stages Studio shows, in the same words. A person who used
 * the site and then downloads Studio must recognise the room. */
const STAGES = [
  { key: "read",  label: "Reading Krate's API" },
  { key: "write", label: "Writing the code" },
  { key: "test",  label: "Testing it" },
  { key: "done",  label: "Finishing up" },
];

/* Lines that make the wait feel like work rather than a hang. Studio earns
 * this by showing the app forming; the words carry the rest. */
const THINKING = {
  read:  ["reading what Krate can do", "checking the shapes it needs"],
  write: ["writing the code", "laying out the screen", "wiring the numbers up"],
  test:  ["opening your app", "clicking through it", "checking it stays open"],
  done:  ["packing it into one file", "signing the permissions"],
};

const IDEAS = [
  { short: "A rate card my client can keep",
    full: "a rate card for my studio -- day rate 800, extra hour 120, rush adds 25 percent; the client types days and extra hours and sees the total. It cannot use the network" },
  { short: "A tip-out for tonight",
    full: "a tip-out for tonight -- staff type the night total; bar gets 40 percent, floor 35, kitchen 25. It cannot use the network" },
  { short: "A lab my students just open",
    full: "a week 3 heat-loss lab for my class -- three checks and a score at the bottom; students open the file, no account. It cannot use the network" },
];

/* ---- the hub ------------------------------------------------------------ */

async function hub(path, opts = {}) {
  const headers = { ...(opts.headers || {}) };
  if (state.token) headers.authorization = `Bearer ${state.token}`;
  if (opts.body && !headers["content-type"]) headers["content-type"] = "application/json";
  const res = await fetch(HUB + path, { ...opts, headers });
  if (!res.ok) throw new Error(await res.text().catch(() => res.statusText));
  const type = res.headers.get("content-type") || "";
  return type.includes("json") ? res.json() : res.text();
}

function loadToken() {
  try { state.token = localStorage.getItem("krate-token"); } catch (e) {}
}

async function loadMe() {
  if (!state.token) return;
  try {
    state.me = await hub("/me");
  } catch (e) {
    // An expired token is not an error worth showing: the page works
    // signed out, and the wall asks for a sign-in when it needs one.
    state.token = null;
    try { localStorage.removeItem("krate-token"); } catch (e2) {}
  }
}

/* ---- what the page is showing ------------------------------------------- */

function show(view) {
  for (const id of ["viewAsk", "viewWork", "viewDone"]) {
    $(id).classList.toggle("hidden", id !== view);
  }
}

/* The entrance. Staggered, short, and only on first paint -- motion that
 * repeats on every state change stops being motion and becomes a wait. */
function riseIn() {
  document.querySelectorAll(".rise").forEach((el, i) => {
    setTimeout(() => el.classList.add("in"), 60 + i * 70);
  });
}

function paintGreeting() {
  const name = state.me && state.me.user && (state.me.user.name || state.me.user.login);
  const first = name ? String(name).trim().split(/\s+/)[0] : "";
  $("greeting").innerHTML = first
    ? `What should we make, ${escapeHtml(first)}<span class="soft">?</span>`
    : `What should we make<span class="soft">?</span>`;
}

function paintAccount() {
  const user = state.me && state.me.user;
  const btn = $("accountBtn");
  if (user && user.avatar_url) {
    btn.innerHTML = `<img src="${escapeAttr(user.avatar_url)}" alt="" />`;
  } else if (user) {
    $("initial").textContent = (user.login || user.name || "?")[0].toUpperCase();
  }
}

function paintIdeas() {
  $("ideas").innerHTML = IDEAS.map((idea, i) => `
    <button class="idea" data-i="${i}">
      <span>${escapeHtml(idea.short)}</span>
      <span class="go">→</span>
    </button>`).join("");
  document.querySelectorAll(".idea").forEach((b) => {
    b.addEventListener("click", () => {
      const box = $("prompt");
      box.value = IDEAS[b.dataset.i].full;
      box.focus();
      box.dispatchEvent(new Event("input"));
    });
  });
}

/* ---- making ------------------------------------------------------------- */

async function startMake() {
  const request = $("prompt").value.trim();
  if (!request) return;
  state.request = request;

  // The wall is the hub's to enforce -- a counter the page owns is a
  // counter anyone can edit. The page's job is to explain the answer.
  if (!state.token) return askToSignIn();

  show("viewWork");
  resetWork();

  try {
    const job = await hub("/build", {
      method: "POST",
      body: JSON.stringify({ request }),
    });
    state.job = job.id;
    poll();
  } catch (err) {
    const message = String(err.message || err);
    if (/three|limit|plan|subscri/i.test(message)) return hitTheWall(message);
    failed(message);
  }
}

function resetWork() {
  state.stage = -1;
  $("workTitle").textContent = "Making your app";
  $("workSub").textContent = "a few minutes";
  $("nowLine").textContent = "warming up";
  $("track").style.transform = "scaleX(0.03)";
  $("shot").classList.add("hidden");
  $("ghost").classList.remove("hidden");
  $("sweep").classList.remove("hidden");
  $("caret").classList.remove("hidden");
  $("stop").classList.remove("hidden");
}

/* Progress comes from the server, but the LINE moves on its own between
 * updates -- a stage that sits still for forty seconds reads as a hang
 * even when the work is fine. */
let thinkTimer = null;

function advance(stageKey, line) {
  const idx = STAGES.findIndex((s) => s.key === stageKey);
  if (idx > state.stage) {
    state.stage = idx;
    $("track").style.transform = `scaleX(${(idx + 0.5) / STAGES.length})`;
  }
  if (line) {
    $("nowLine").textContent = line;
  } else {
    const lines = THINKING[stageKey] || THINKING.read;
    let i = 0;
    clearInterval(thinkTimer);
    $("nowLine").textContent = lines[0];
    thinkTimer = setInterval(() => {
      i = (i + 1) % lines.length;
      $("nowLine").textContent = lines[i];
    }, 4200);
  }
}

async function poll() {
  if (!state.job) return;
  let job;
  try {
    job = await hub(`/build/${state.job}`);
  } catch (err) {
    return failed(String(err.message || err));
  }

  if (job.stage) advance(job.stage, job.line);

  // The app's own first frame, the moment it exists. This is the whole
  // reason the wait is watchable: the skeleton becomes the real thing.
  if (job.shot) {
    const img = $("shot");
    if (img.dataset.src !== job.shot) {
      img.dataset.src = job.shot;
      img.src = job.shot;
      img.onload = () => {
        img.classList.remove("hidden");
        $("ghost").classList.add("hidden");
        $("sweep").classList.add("hidden");
      };
    }
  }

  if (job.state === "done") return finished(job.result);
  if (job.state === "failed") return failed(job.error || "that one didn't come together");
  setTimeout(poll, 1500);
}

function finished(result) {
  clearInterval(thinkTimer);
  state.result = result;
  state.job = null;

  $("doneName").textContent = result.name || "Your app";
  $("doneSub").textContent = [result.size, "Mac, Windows and Linux"].filter(Boolean).join(" · ");
  $("asks").innerHTML = (result.asks || []).slice(0, 2)
    .map((a) => `<li>asks ${escapeHtml(a)}</li>`).join("");

  const frame = $("doneFrame");
  const img = $("doneShot");
  if (result.shot) {
    img.src = result.shot;
    frame.classList.remove("hidden");
  } else {
    // No still is not a "?" placeholder. The frame goes and the app's
    // name and buttons stay, which is what a person came for.
    frame.classList.add("hidden");
  }
  $("doneNote").textContent = "";
  show("viewDone");
}

function failed(message) {
  clearInterval(thinkTimer);
  state.job = null;
  $("workTitle").textContent = "That one didn't come together";
  $("workSub").textContent = "Your words are still here.";
  $("nowLine").textContent = message.slice(0, 160);
  $("caret").classList.add("hidden");
  $("sweep").classList.add("hidden");
  $("stop").textContent = "Try again";
  $("stop").onclick = () => { $("stop").textContent = "Stop"; $("stop").onclick = stopBuild; startMake(); };
}

function stopBuild() {
  clearInterval(thinkTimer);
  if (state.job) hub(`/build/${state.job}/stop`, { method: "POST" }).catch(() => {});
  state.job = null;
  show("viewAsk");
}

/* ---- the wall, the account, the shelf ----------------------------------- */

function sheet(html) {
  const wrap = document.createElement("div");
  wrap.className = "sheet-wrap";
  wrap.innerHTML = `<div class="sheet">${html}</div>`;
  wrap.addEventListener("click", (e) => { if (e.target === wrap) wrap.remove(); });
  document.addEventListener("keydown", function esc(e) {
    if (e.key === "Escape") { wrap.remove(); document.removeEventListener("keydown", esc); }
  });
  document.body.appendChild(wrap);
  return wrap;
}

function askToSignIn() {
  const wrap = sheet(`
    <h3>One quick sign-in</h3>
    <p>So your apps are yours, and so we know which three are free.</p>
    <div class="rows">
      <button class="row" id="signGh"><b>Continue with GitHub</b></button>
      <button class="row" id="signGoogle"><b>Continue with Google</b></button>
    </div>
    <p class="note">Krate never sees your password. Three apps free, then $12 a month.</p>
    <button class="close" data-close>Not now</button>`);
  wrap.querySelector("#signGh").onclick = () => signIn("/login/start");
  wrap.querySelector("#signGoogle").onclick = () => signIn("/login/google/start");
  wrap.querySelector("[data-close]").onclick = () => wrap.remove();
}

function signIn(path) {
  // The browser hand-off, not a code to retype: they come back signed in.
  const back = encodeURIComponent(location.origin + location.pathname);
  location.href = `${HUB}${path}?return=${back}`;
}

function hitTheWall(message) {
  show("viewAsk");
  const plan = (state.me && state.me.plan) || {};
  const wrap = sheet(`
    <h3>That's the three free ones</h3>
    <p>${escapeHtml(message || "Every app you have made is yours to keep, whatever happens next.")}</p>
    <div class="rows">
      <button class="row" id="goStudio">
        <span><b>Studio, $12 a month</b><small>Unlimited apps. Or $96 for the year.</small></span>
      </button>
      <button class="row" id="goFounding">
        <span><b>Founding 200, $79 a year</b><small>Locked in for as long as you stay.</small></span>
      </button>
    </div>
    <p class="note">${plan.active ? "" : "Changes to an app and builds that fail never count."}</p>
    <button class="close" data-close>Not now</button>`);
  wrap.querySelector("#goStudio").onclick = () => checkout("monthly");
  wrap.querySelector("#goFounding").onclick = () => checkout("founding");
  wrap.querySelector("[data-close]").onclick = () => wrap.remove();
}

async function checkout(plan) {
  try {
    const out = await hub("/billing/checkout", {
      method: "POST",
      body: JSON.stringify({ plan, return_url: location.href }),
    });
    if (out.url) location.href = out.url;
  } catch (err) {
    alert(String(err.message || err));
  }
}

async function openAccount() {
  if (!state.token) return askToSignIn();
  const user = (state.me && state.me.user) || {};
  const plan = (state.me && state.me.plan) || {};
  const planWords = plan.active
    ? (plan.plan === "founding" ? "Founding 200" : "Studio")
    : "Free";

  const wrap = sheet(`
    <h3>${escapeHtml(user.name || user.login || "You")}</h3>
    <p>${escapeHtml(user.email || user.login || "")}</p>
    <div class="rows">
      <button class="row" id="rowPlan"><span><b>Plan</b></span><span class="val">${planWords}</span></button>
      <button class="row" id="rowApps"><span><b>Your apps</b><small>everything you have made</small></span></button>
      ${plan.portal ? `<button class="row" id="rowBilling"><span><b>Billing</b><small>card, invoices, cancel</small></span></button>` : ""}
    </div>
    <p class="note">Every app you make is a file that is yours forever.</p>
    <button class="close" id="signOut">Sign out</button>`);

  wrap.querySelector("#rowPlan").onclick = () => { wrap.remove(); hitTheWall(""); };
  wrap.querySelector("#rowApps").onclick = () => { wrap.remove(); openApps(); };
  const billing = wrap.querySelector("#rowBilling");
  if (billing) billing.onclick = async () => {
    try {
      const out = await hub("/billing/portal", { method: "POST", body: JSON.stringify({ return_url: location.href }) });
      if (out.url) location.href = out.url;
    } catch (err) { alert(String(err.message || err)); }
  };
  wrap.querySelector("#signOut").onclick = () => {
    try { localStorage.removeItem("krate-token"); } catch (e) {}
    location.reload();
  };
}

async function openApps() {
  const wrap = sheet(`
    <h3>Your apps</h3>
    <p>Everything you have made. Each one is a file you already own.</p>
    <div class="shelf" id="shelf"><p class="empty">reading…</p></div>
    <button class="close" data-close>Close</button>`);
  wrap.querySelector("[data-close]").onclick = () => wrap.remove();

  try {
    const out = await hub("/my/apps");
    const apps = out.apps || out || [];
    const shelf = wrap.querySelector("#shelf");
    if (!apps.length) {
      shelf.innerHTML = `<p class="empty">Nothing yet. The box is waiting.</p>`;
      return;
    }
    shelf.innerHTML = apps.map((a) => {
      const meta = a.meta || a;
      return `<button data-url="${escapeAttr(a.url || "")}">
        <span>${escapeHtml(meta.name || "App")}</span>
        <span class="sz">${prettySize(meta.size)}</span>
      </button>`;
    }).join("");
    shelf.querySelectorAll("button").forEach((b) => {
      b.onclick = () => { if (b.dataset.url) window.open(b.dataset.url, "_blank", "noopener"); };
    });
  } catch (err) {
    wrap.querySelector("#shelf").innerHTML =
      `<p class="empty">Could not read them just now.</p>`;
  }
}

/* ---- small helpers ------------------------------------------------------ */

function prettySize(size) {
  if (typeof size === "number" && size > 0) {
    return size < 1024 * 1024
      ? Math.round(size / 1024) + " KB"
      : (size / (1024 * 1024)).toFixed(1) + " MB";
  }
  return typeof size === "string" ? size : "";
}

function escapeHtml(s) {
  return String(s).replace(/[&<>"']/g, (c) =>
    ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
}
function escapeAttr(s) { return escapeHtml(s); }

/* ---- wiring ------------------------------------------------------------- */

function grow(el) {
  el.style.height = "auto";
  el.style.height = Math.min(el.scrollHeight, 190) + "px";
}

function boot() {
  loadToken();

  // A sign-in hands the token back on the URL. Take it, store it, and get
  // it out of the address bar so nobody copies a link with their session
  // in it.
  const params = new URLSearchParams(location.search);
  const handed = params.get("token");
  if (handed) {
    state.token = handed;
    try { localStorage.setItem("krate-token", handed); } catch (e) {}
    history.replaceState({}, "", location.pathname);
  }

  paintIdeas();
  show("viewAsk");
  riseIn();

  const box = $("prompt");
  box.addEventListener("input", () => {
    grow(box);
    $("send").classList.toggle("ready", Boolean(box.value.trim()));
  });
  box.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && !e.shiftKey && !e.isComposing) { e.preventDefault(); startMake(); }
  });
  setTimeout(() => box.focus(), 220);

  $("send").onclick = startMake;
  $("stop").onclick = stopBuild;
  $("accountBtn").onclick = openAccount;

  $("again").onclick = () => {
    $("prompt").value = "";
    $("send").classList.remove("ready");
    show("viewAsk");
    $("prompt").focus();
  };
  $("change").onclick = () => {
    show("viewAsk");
    const b = $("prompt");
    b.value = state.request;
    b.focus();
    b.setSelectionRange(b.value.length, b.value.length);
    b.dispatchEvent(new Event("input"));
  };
  $("download").onclick = () => {
    if (state.result && state.result.download) location.href = state.result.download;
  };
  $("sendLink").onclick = async () => {
    if (!state.result) return;
    if (state.result.share) {
      await copy(state.result.share);
      $("doneNote").textContent = "Link copied. Anyone who opens it gets the app.";
      return;
    }
    $("doneNote").textContent = "Publishing…";
    try {
      const out = await hub("/publish/from-build", {
        method: "POST",
        body: JSON.stringify({ id: state.result.id }),
      });
      state.result.share = out.url;
      await copy(out.url);
      $("doneNote").textContent = "Link copied. Anyone who opens it gets the app.";
    } catch (err) {
      $("doneNote").className = "note bad";
      $("doneNote").textContent = String(err.message || err);
    }
  };

  loadMe().then(() => { paintGreeting(); paintAccount(); });
}

async function copy(text) {
  try { await navigator.clipboard.writeText(text); } catch (e) {}
}

boot();
