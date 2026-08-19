/* Krate Studio, the front of it.
 *
 * Three views -- gate, home, session -- and one build state machine:
 * idle -> building -> done | failed. The backend is the krate engine spawned
 * by the Tauri shell; every line it prints streams in as an event. In a
 * plain browser (no Tauri) a mock backend stands in, so the design can be
 * seen and judged without building the shell.
 *
 * Sessions are whole conversations saved as JSON through the shell. Coming
 * back to one restores the thread and the finished app, and the next
 * message revises that same app -- the "someone told me my timer needs a
 * reset button, let me go fix that" path.
 */

"use strict";

const tauri = window.__TAURI__ || null;
const $ = (id) => document.getElementById(id);
const invoke = (cmd, args) => (tauri ? tauri.core.invoke(cmd, args) : mockInvoke(cmd, args));

/* One update check per session: newer release -> a quiet chip that opens
 * this machine's installer. No auto-download, no nagging; the person
 * clicks when they want it. */
(async function () {
  try {
    const mine = await invoke("studio_version");
    const r = await fetch("https://api.github.com/repos/incyashraj/krate/releases/latest");
    if (!r.ok) return;
    const rel = await r.json();
    const latest = String(rel.tag_name || "").replace(/^v/, "");
    if (!latest || latest === mine) return;
    const newer = latest.localeCompare(mine, undefined, { numeric: true }) > 0;
    if (!newer) return;
    const ua = navigator.userAgent;
    const file = ua.includes("Windows")
      ? `krate-studio-${latest}-windows-x64-setup.exe`
      : ua.includes("Linux") && !ua.includes("Android")
        ? `krate-studio-${latest}-linux-x86_64.AppImage`
        : `krate-studio-${latest}-universal.dmg`;
    const chip = $("updateChip");
    chip.textContent = `Update to v${latest}`;
    chip.classList.remove("hidden");
    chip.addEventListener("click", () => {
      invoke("open_external", {
        url: `https://github.com/incyashraj/krate/releases/download/v${latest}/${file}`,
      }).catch(() => {});
    });
  } catch (e) { /* the check is a courtesy, never a failure */ }
})();

/* The OS shapes the chrome: macOS keeps its traffic lights (the bar reserves
 * their run), Windows loses the native frame and gets our three buttons,
 * Linux keeps its native frame but has no lights to clear. */
(function () {
  const ua = navigator.userAgent;
  if (ua.includes("Windows")) {
    document.body.classList.add("windows");
    $("winControls").classList.remove("hidden");
    $("winMin").addEventListener("click", () => invoke("win_minimize").catch(() => {}));
    $("winMax").addEventListener("click", () => invoke("win_toggle_max").catch(() => {}));
    $("winClose").addEventListener("click", () => invoke("win_close").catch(() => {}));
  } else if (ua.includes("Linux")) {
    document.body.classList.add("linux");
  }
})();

const STAGES = [
  { key: "think", label: "Understanding what you asked for" },
  { key: "write", label: "Writing the code" },
  { key: "build", label: "Building it" },
  { key: "pack",  label: "Packing it into one file" },
  { key: "wall",  label: "Checking it only touches what it declared" },
];

const state = {
  phase: "idle",
  session: null,        // { id, title, created, updated, messages, result }
  attachments: [],      // absolute paths staged for the next message
  planning: null,       // the pre-build conversation (K-123): request, qa, files
  watchdog: null,       // proves a build is alive, never just spinning (K-131)
  sawEngineLine: false, // has the engine said anything at all this build?
  lastRequest: null,    // what to retry if the watchdog ends a dead build
  cloud: [],            // apps read from Krate Cloud
  cloudCat: "all",      // which category chip is selected
  cloudApp: null,       // the published app being looked at
  agents: [],
  agent: "claude",
  outDir: "",
  account: null,
  startedAt: 0,
  timer: null,
  stageIndex: -1,
  lastFailed: null,     // the message to retry
};

/* ---- views ------------------------------------------------------------ */

function showView(name) {
  for (const id of ["viewGate", "viewHome", "viewSession", "viewCloud", "viewApp"]) {
    $(id).classList.add("hidden");
  }
  const view = $({
    gate: "viewGate", home: "viewHome", session: "viewSession",
    cloud: "viewCloud", appDetail: "viewApp",
  }[name]);
  view.classList.remove("hidden");
  revealIn(view);
}

/* The site's one motion idiom: translateY(14px) + opacity over 0.5s, on a
 * stagger. The site drives it from scroll position; a desktop app swaps
 * views instead, so each view replays its own stagger when it appears.
 * 60ms apart is enough to read as a sequence without anyone waiting. */
function revealIn(root) {
  if (matchMedia("(prefers-reduced-motion: reduce)").matches) {
    root.querySelectorAll(".reveal").forEach((el) => el.classList.add("in"));
    return;
  }
  const items = [...root.querySelectorAll(".reveal")];
  // Inline, so nothing in the cascade can strand an element invisible.
  items.forEach((el) => {
    el.style.opacity = "0";
    el.style.transform = "translateY(14px)";
  });
  const showAll = () =>
    items.forEach((el) => {
      el.style.opacity = "1";
      el.style.transform = "none";
    });
  // A safety net, because the failure mode here is a blank screen.
  clearTimeout(root._revealGuard);
  root._revealGuard = setTimeout(showAll, items.length * 60 + 900);
  // Two frames: the first paints the from-state, the second starts the
  // transition. One frame is not enough and the element simply appears.
  requestAnimationFrame(() =>
    requestAnimationFrame(() => {
      items.forEach((el, i) =>
        setTimeout(() => {
          el.style.opacity = "1";
          el.style.transform = "none";
        }, i * 60),
      );
    }),
  );
}

/* ---- the gate --------------------------------------------------------- */

async function boot() {
  try {
    const account = await invoke("account_status");
    if (account && account.signed_in) {
      state.account = account;
      enterHome();
      return;
    }
  } catch (err) {
    // A missing engine shows on the gate too -- there is nowhere better,
    // and a person facing a broken install needs the real reason rather
    // than a sign-in button that can never work.
    $("gateError").textContent = plainWords(err);
    $("gateError").classList.remove("hidden");
    $("loginBtn").disabled = true;
  }
  showView("gate");
}

async function login() {
  $("loginBtn").disabled = true;
  $("gateError").classList.add("hidden");
  try {
    await invoke("account_login");
    // The "done" step handler flips us in; this resolves after it.
  } catch (err) {
    $("gateStart").classList.remove("hidden");
    $("gateCode").classList.add("hidden");
    $("gateError").textContent = signInWords(err);
    $("gateError").classList.remove("hidden");
  } finally {
    $("loginBtn").disabled = false;
  }
}

function onLoginStep(step) {
  if (step.step === "code") {
    $("gateStart").classList.add("hidden");
    $("gateCode").classList.remove("hidden");
    $("gateCodeValue").textContent = step.code;
    // One click puts the code on the clipboard: on Windows the code was
    // half-buried and not selectable, and eight characters you cannot copy
    // is eight chances to mistype.
    $("gateCodeValue").onclick = async () => {
      try {
        await navigator.clipboard.writeText(step.code);
        $("gateCodeValue").classList.add("copied");
        setTimeout(() => $("gateCodeValue").classList.remove("copied"), 1200);
      } catch {}
    };
    $("gateUrl").textContent = step.url;
  } else if (step.step === "adopted") {
    // The identity was stored by the engine (browser hand-off); read it
    // back rather than trusting the URL's own fields.
    refreshAccountAndEnter();
  } else if (step.step === "done") {
    state.account = { signed_in: true, login: step.login, name: step.name, avatar_url: step.avatar_url };
    enterHome();
  } else if (step.step === "error") {
    $("gateStart").classList.remove("hidden");
    $("gateCode").classList.add("hidden");
    $("gateError").textContent = step.why;
    $("gateError").classList.remove("hidden");
  }
}

/* ---- home ------------------------------------------------------------- */

async function enterHome() {
  showView("home");
  renderAccount();
  // Settings FIRST, then agents -- and await both.
  //
  // These two lines were the other way round and refreshAgents() was not
  // awaited, so two things went wrong at once: a request submitted before the
  // async agent probe returned used the default "claude", and when the probe
  // did return, the settings load overwrote its choice. The chip painted
  // "Codex" from the probe while state.agent still said "claude", and the
  // build failed in the next second with "the `claude` command is not
  // installed" on a machine where Codex was installed and working.
  const settings = await invoke("settings_get");
  state.outDir = settings.out_dir;
  state.agent = settings.agent || "claude";
  await refreshAgents();
  renderSessions(await invoke("sessions_list"));
  renderBuilding();
}

/* A build keeps running while you browse. Without this the home screen looked
 * idle, the next request was refused with "one app is already being made",
 * and there was no way to reach the build to stop it. */
function renderBuilding() {
  const bar = $("buildingNow");
  if (!bar) return;
  // Keyed on the building session alone, never on which view is showing:
  // opening another session used to smash the shared phase flag and make a
  // live build vanish from "making now" while it was still running.
  const live = Boolean(state.buildingSession);
  bar.classList.toggle("hidden", !live);
  if (live) $("buildingNowTitle").textContent = state.buildingSession.title;
}

function renderAccount() {
  const a = state.account;
  if (!a) return;
  if (a.avatar_url) {
    $("avatar").src = a.avatar_url;
    $("avatar").classList.remove("hidden");
    $("accountInitial").textContent = "";
  } else {
    $("accountInitial").textContent = (a.login || "?")[0].toUpperCase();
  }
}

function renderSessions(sessions) {
  const grid = $("appsGrid");
  grid.innerHTML = "";
  // Cards are added below with .reveal; stagger them once the grid is built.
  setTimeout(() => revealIn(grid), 0);
  if (!sessions.length) {
    grid.innerHTML = `<p class="apps-empty">Nothing yet. Your first app is one sentence away.</p>`;
    return;
  }
  for (const s of sessions) {
    const card = document.createElement("button");
    card.className = "app-card";
    const size = s.result && s.result.size ? ` · ${s.result.size}` : "";
    const hasShot = s.result && s.result.shot;
    card.innerHTML = `<div class="thumb-well${hasShot ? "" : " blank"}"></div>
      <div class="card-body"><p class="name"></p><p class="meta">${timeAgo(s.updated)}${size}</p></div>`;
    const well = card.querySelector(".thumb-well");
    if (hasShot) {
      const img = document.createElement("img");
      img.src = s.result.shot;
      img.alt = "";
      well.appendChild(img);
    } else {
      well.textContent = "open to pick up where you left off";
    }
    card.querySelector(".name").textContent = s.title;
    card.addEventListener("click", () => openSession(s));

    const x = document.createElement("button");
    x.className = "card-x";
    x.textContent = "×";
    x.title = "Remove from your apps";
    x.addEventListener("click", async (e) => {
      // The card underneath opens a session; this must not do both.
      e.stopPropagation();
      await invoke("session_delete", { id: s.id });
      renderSessions(await invoke("sessions_list"));
    });
    card.appendChild(x);
    card.classList.add("reveal");
    grid.appendChild(card);
  }
}

function timeAgo(secs) {
  const d = Math.floor(Date.now() / 1000) - secs;
  if (d < 90) return "just now";
  if (d < 3600) return `${Math.floor(d / 60)} min ago`;
  if (d < 86400 * 2) return `${Math.floor(d / 3600)} h ago`;
  return `${Math.floor(d / 86400)} days ago`;
}

/* ---- sessions --------------------------------------------------------- */

function newSession(firstRequest) {
  const now = Math.floor(Date.now() / 1000);
  state.session = {
    id: `s-${Date.now()}`,
    title: firstRequest.slice(0, 60),
    created: now,
    updated: now,
    messages: [],
    result: null,
  };
}

function openSession(s) {
  // A build can finish on disk after the window is gone. The shell records
  // the target path before it starts, so a session with no result but a
  // pending path may have a finished app waiting -- adopt it rather than
  // calling it unfinished and orphaning the app.
  if (!s.result && s.pending_path) {
    s.result = {
      path: s.pending_path,
      name: s.pending_path.split(/[\\/]/).pop(),
      size: "",
      asks: [],
      shot: "",
      recovered: true,
    };
  }
  // Reopening the session that is building right now must land back on the
  // live progress, not on "your app will appear here". The stage list, log
  // and timer are all still wired to it; use the building session's own
  // object so its transcript keeps growing in one place.
  const building = state.buildingSession && state.buildingSession.id === s.id;
  state.session = building ? state.buildingSession : s;
  state.attachments = [];
  $("railTitle").textContent = state.session.title;
  $("thread").innerHTML = "";
  for (const m of state.session.messages) {
    appendMessage(m.who, m.body, m.files);
  }
  if (building) {
    show("building");
  } else if (state.session.result) {
    fillDone(state.session.result, { reveal: false });
    show("done");
    setRevisePlaceholders();
  } else {
    show("idle");
  }
  showView("session");
  $("prompt").focus();
}

async function persist() {
  return persistSession(state.session);
}

async function persistSession(s) {
  if (!s) return;
  s.updated = Math.floor(Date.now() / 1000);
  try {
    await invoke("session_save", { session: s });
  } catch (e) {
    /* history is a convenience; never let saving break making */
  }
}

/* ---- thread ----------------------------------------------------------- */

function appendMessage(who, body, files, extra) {
  const el = document.createElement("div");
  const variant = extra && extra.variant ? ` ${extra.variant}` : "";
  el.className = `msg ${who === "KRATE" ? "krate" : ""}${variant}`;
  el.innerHTML = `<span class="who">${who}</span><span class="body"></span>`;
  el.querySelector(".body").textContent = body;
  if (files && files.length) {
    const f = document.createElement("span");
    f.className = "files";
    f.textContent = `📎 ${files.map(baseName).join(", ")}`;
    el.appendChild(f);
  }
  // Action buttons IN the thread: the escape from a question is a click,
  // never a magic phrase the person has to know.
  if (extra && extra.actions && extra.actions.length) {
    const row = document.createElement("span");
    row.className = "msg-actions";
    for (const action of extra.actions) {
      const btn = document.createElement("button");
      btn.className = action.primary ? "btn btn-primary msg-btn" : "btn msg-btn";
      btn.textContent = action.label;
      btn.addEventListener("click", () => {
        row.remove();
        action.run();
      });
      row.appendChild(btn);
    }
    el.appendChild(row);
  }
  $("thread").appendChild(el);
  $("thread").scrollTop = $("thread").scrollHeight;
}

/* A live build chip on the timeline: v3 building · <phase>. Returns the
   element so finish/fail can settle it into a receipt. Not recorded in the
   transcript while live; the settled receipt is. */
function appendLiveChip(version) {
  const el = document.createElement("div");
  el.className = "msg krate vlive";
  el.innerHTML = `<span class="who">KRATE</span><span class="vchip vlivec"><b>v${version}</b> building <span class="vbar"><i style="transform:scaleX(0.08)"></i></span> <span class="vm" data-phase>starting…</span></span>`;
  // Stop lives on the build itself, not only on the far side of the window.
  const stop = document.createElement("button");
  stop.className = "vact vg";
  stop.textContent = "Stop";
  stop.addEventListener("click", stopBuild);
  el.querySelector(".vchip").appendChild(stop);
  $("thread").appendChild(el);
  $("thread").scrollTop = $("thread").scrollHeight;
  return el;
}

function settleChipOk(el, version, sizeLabel, minsLabel, app) {
  if (!el) return;
  el.className = "msg krate vok";
  el.innerHTML = `<span class="who">KRATE</span><span class="vchip"><b>v${version}</b> built <span class="vm">${sizeLabel}${minsLabel}</span></span>`;
  const chip = el.querySelector(".vchip");
  const open = document.createElement("button");
  open.className = "vact";
  open.textContent = "Open";
  open.addEventListener("click", openApp);
  chip.appendChild(open);
  const share = document.createElement("button");
  share.className = "vact vg";
  share.textContent = "Share";
  share.addEventListener("click", openPublishSheet);
  chip.appendChild(share);
}

function settleChipBad(el, version, retry) {
  if (!el) return;
  el.className = "msg krate vbad";
  el.innerHTML = `<span class="who">KRATE</span><span class="vchip vbadc"><b>v${version}</b> failed <span class="vm">app untouched</span></span>`;
  if (retry) {
    const fix = document.createElement("button");
    fix.className = "vact";
    fix.textContent = "Try again";
    fix.addEventListener("click", retry);
    el.querySelector(".vchip").appendChild(fix);
  }
}

function say(who, body, files, extra) {
  // Always the visible session by definition; drawn live with its buttons,
  // recorded without them -- a reopened transcript shows only the words.
  if (!state.session) return;
  appendMessage(who, body, files, extra);
  state.session.messages.push({
    who,
    body,
    files: files || [],
    when: Math.floor(Date.now() / 1000),
  });
}

/* Record a line in a specific session, drawing it only when that session is
 * the one on screen. A build keeps talking after the person walks away; its
 * words must land in ITS transcript, not whichever thread they now look at. */
function sayTo(session, who, body, files) {
  if (!session) return;
  if (state.session && state.session.id === session.id) appendMessage(who, body, files);
  session.messages.push({ who, body, files: files || [], when: Math.floor(Date.now() / 1000) });
}

const baseName = (p) => p.split(/[\\/]/).pop();

/* ---- build state machine ---------------------------------------------- */

function show(phase) {
  // Leaving idle for anything else means the note has done its job; the
  // next visit to idle starts neutral again unless a caller sets it.
  if (phase !== "idle") setIdleNote("Your app will appear here.");
  state.phase = phase;
  for (const id of ["stateIdle", "stateBuilding", "stateDone", "stateFailed"]) {
    $(id).classList.add("hidden");
  }
  $({ idle: "stateIdle", building: "stateBuilding", done: "stateDone", failed: "stateFailed" }[phase])
    .classList.remove("hidden");
}

/* Never let a spinner outlive its build.
 *
 * Two independent guarantees, because they catch different failures:
 *
 *   1. LIVENESS -- every few seconds, ask the backend whether a process is
 *      actually running. A build whose engine died unseen (crash, sleep, a
 *      kill from outside) used to leave this screen spinning forever while
 *      a first-time person believed their app was being made.
 *   2. PROGRESS -- if nothing at all has been heard from the engine within
 *      the first ninety seconds, something is wrong with the plumbing
 *      rather than with the app; say so instead of animating.
 *
 * Both end the build the honest way, through the normal failure path, so
 * the words are the same ones the person would get from any other failure.
 */
function startBuildWatchdog() {
  clearInterval(state.watchdog);
  const started = Date.now();
  state.sawEngineLine = false;
  state.watchdog = setInterval(async () => {
    if (!state.buildingSession) {
      clearInterval(state.watchdog);
      return;
    }
    const age = Date.now() - started;
    // Give the engine a moment to spawn before asking about it.
    if (age < 6000) return;
    let alive = true;
    try {
      alive = await invoke("build_alive");
    } catch (err) {
      return; // a failed question is not an answer; try again next tick
    }
    if (!alive) {
      // The engine process is gone -- but "gone" is how BOTH a crash and a
      // normal finish look: create exits either way. The create promise is
      // the one that knows which, because it has the exit code and the result;
      // it settles the build a beat later. So the watchdog must NOT call this a
      // failure. It stops watching and yields. If the finish was real,
      // finishBuild fills the done card. If the process truly died without a
      // result, the create promise rejects and failBuild runs from the catch.
      //
      // The old behavior raced that promise: a watchdog tick landing in the
      // gap between the process exiting and the promise resolving settled the
      // chip to "failed" while the right panel then showed the completed app --
      // the exact left-says-failed / right-shows-done contradiction the founder
      // hit. Leaving the verdict to the promise removes the race entirely.
      clearInterval(state.watchdog);
      return;
    }
    if (!state.sawEngineLine && age > 90000) {
      clearInterval(state.watchdog);
      invoke("stop_build").catch(() => {});
      failBuild(
        "The build never got started -- Krate's engine went quiet before it " +
          "said anything. Trying again usually works.",
        state.lastRequest || "",
      );
    }
  }, 4000);
}

function beginBuild(title, expect) {
  $("buildTitle").textContent = title;
  $("buildExpect").textContent = expect;
  $("stages").innerHTML = STAGES.map(
    (s) => `<li data-key="${s.key}"><span class="tick"></span>${s.label}</li>`,
  ).join("");
  $("buildLog").textContent = "";
  const fill0 = $("buildFill");
  if (fill0) fill0.style.width = "4%";
  state.stageIndex = -1;
  advanceStage("think");
  state.startedAt = Date.now();
  clearInterval(state.timer);
  state.lastLineAt = Date.now();
  let thinkIdx = 0;
  state.timer = setInterval(() => {
    const s = Math.floor((Date.now() - state.startedAt) / 1000);
    $("elapsed").textContent = `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
    if (Date.now() - state.lastLineAt > 18000) {
      $("nowLine").textContent = THINKING[thinkIdx++ % THINKING.length];
      state.lastLineAt = Date.now() - 8000; // rotate every ~10s while quiet
    }
  }, 1000);
  $("nowLine").textContent = "warming up…";
  show("building");
}

const STAGE_SAID = {
  build: "The code is written. Building it now.",
  wall: "Built. Checking it only touches what it declared.",
};

/* Lines that mean "a window is about to appear on your screen". When one of
   these is the live step, the build card says so plainly, because the flash
   and the sound arrive with it. */
const FLASH_WORDS = /opening your app|running your app|looking at how your app/i;

function advanceStage(key) {
  const idx = STAGES.findIndex((s) => s.key === key);
  if (idx <= state.stageIndex) return;
  state.stageIndex = idx;
  // Two milestones worth saying out loud. Not five -- a chat that narrates
  // every step is noise, and the stage list already shows all of them.
  if (STAGE_SAID[key]) sayTo(state.buildingSession || state.session, "KRATE", STAGE_SAID[key]);
  document.querySelectorAll("#stages li").forEach((li, i) => {
    li.className = i < idx ? "done" : i === idx ? "now" : "";
  });
  // The live detail belongs visually to the step doing the work.
  const peek = $("peekBox");
  const current = document.querySelector("#stages li.now");
  if (peek && current) current.insertAdjacentElement("afterend", peek);
  // One line at eye level says where we are; the full list stays one click
  // away for anyone who wants it.
  const nowStage = $("nowStage");
  if (nowStage) nowStage.textContent = STAGES[idx].label;
  const stepCount = $("stepCount");
  if (stepCount) stepCount.textContent = `step ${idx + 1} of ${STAGES.length}`;
  if (state.buildChip) {
    const phase = state.buildChip.querySelector("[data-phase]");
    if (phase) phase.textContent = STAGES[idx].label.toLowerCase();
    const bar = state.buildChip.querySelector(".vbar i");
    if (bar) bar.style.transform = `scaleX(${(idx + 0.5) / STAGES.length})`;
  }
  setProgress((idx + 0.5) / STAGES.length);
}

/* The bar tracks real stages, never a timer pretending to know how long an
 * AI will think. It stops at 92% until the app actually exists, because a
 * bar that sits at 100% while nothing has finished is a lie the person can
 * see through -- and this build genuinely takes minutes. */
function setProgress(fraction) {
  const pct = Math.max(4, Math.min(92, Math.round(fraction * 100)));
  const fill = $("buildFill");
  if (fill) fill.style.width = pct + "%";
}

/* Map the engine's own lines onto the stage story. These are our lines,
 * printed by our CLI -- if one changes, the worst case is a stage advancing
 * late, never a wrong claim. */
function onEngineLine(line) {
  // Any word from the engine is proof the plumbing works; the watchdog
  // only fires when we have heard nothing at all.
  state.sawEngineLine = true;
  return onEngineLineInner(line);
}

function onEngineLineInner(line) {
  const log = $("buildLog");
  log.textContent += line + "\n";
  log.scrollTop = log.scrollHeight;
  const clean = line.replace(/^=+>\s*/, "").trim();
  if (clean) {
    $("nowLine").textContent = clean;
    state.lastLineAt = Date.now();
    // A window is about to appear (or just did). Mark the card so the flash
    // and the sound have a visible explanation at the moment they happen.
    const card = document.querySelector("#stateBuilding .build-card");
    if (card) card.classList.toggle("flashing", FLASH_WORDS.test(clean));
  }
  if (/authoring|writing (the|your) app|starter|asking|agent|changing the app/i.test(line)) advanceStage("write");
  if (/==> building|Compiling|Generating bindings/i.test(line)) advanceStage("build");
  if (/==> packing/.test(line)) advanceStage("pack");
  if (/==> verifying/.test(line)) advanceStage("wall");
}

/* When the engine goes quiet -- an AI thinking is real silence -- the
 * heartbeat keeps beating with honest words, so quiet never looks dead. */
const THINKING = [
  "the AI is reading and thinking - this part is quiet",
  "still working - the writing shows up here when it starts",
  "big thoughts take a minute or two",
  "still at it - nothing is stuck",
];

function fillDone(result, opts) {
  $("doneName").textContent = result.name;
  $("doneSize").textContent = result.size;
  $("asks").innerHTML = (result.asks || []).map((a) => `<li>${friendlyAsk(a)}</li>`).join("");
  const shot = $("shot");
  const stage = $("shot").parentElement;
  if (result.shot) {
    shot.src = result.shot;
    shot.classList.remove("hidden");
    stage.classList.remove("no-shot");
    // A data URL that fails to decode would otherwise leave the broken
    // image glyph exactly where the app should be.
    shot.onerror = () => { shot.classList.add("hidden"); stage.classList.add("no-shot"); };
  } else {
    shot.removeAttribute("src");
    shot.classList.add("hidden");
    stage.classList.add("no-shot");
  }
  $("shareResult").classList.toggle("hidden", !result.share_url);
  $("shareResult").classList.remove("error");
  if (result.share_url) {
    $("shareLink").textContent = result.share_url;
    $("shareCopied").classList.add("hidden");
  }
  const card = $("doneCard");
  card.classList.remove("in");
  card.style.opacity = "";
  card.style.transform = "";
  if (opts && opts.reveal) {
    // Arriving now: play the reveal and the one sheen pass.
    card.style.opacity = "0";
    card.style.transform = "translateY(14px)";
    requestAnimationFrame(() =>
      requestAnimationFrame(() => {
        card.classList.add("in");
        card.style.opacity = "1";
        card.style.transform = "none";
      }),
    );
  }
}

function finishBuild(result) {
  // Settle once. If the watchdog already failed this build, ignore the late
  // success rather than drawing a done card behind a failed chip.
  if (state.buildSettled) return;
  state.buildSettled = true;
  clearInterval(state.timer);
  clearInterval(state.watchdog);
  // The result belongs to the session that was building, which is not
  // always the one on screen -- a person can browse other sessions while
  // the AI works. Attaching to state.session put finished apps on the
  // wrong session's card.
  const built = state.buildingSession || state.session;
  built.result = result;
  const mins = Math.round((Date.now() - state.startedAt) / 60000);
  const version = state.buildVersion || (built.builds || 0) + 1;
  built.builds = version;
  settleChipOk(state.buildChip, version, result.size, mins ? ` · ${mins} min` : "", built);
  state.buildChip = null;
  // The transcript keeps the receipt, not the live chip.
  built.messages.push({
    who: "KRATE",
    body: `v${version} built · ${result.size}${mins ? ` · ${mins} min` : ""}. Tell me what to change and it becomes v${version + 1}.`,
    files: [],
    when: Math.floor(Date.now() / 1000),
  });
  persistSession(built);
  const watching = state.session && state.session.id === built.id;
  if (watching) {
    document.querySelectorAll("#stages li").forEach((li) => {
      li.className = "done";
    });
    setProgress(1);
    const fill = $("buildFill");
    if (fill) fill.style.width = "100%";
    fillDone(result, { reveal: true });
    show("done");
    setRevisePlaceholders();
  } else if (!$("viewHome").classList.contains("hidden")) {
    // On the home screen: the finished app should appear among the cards
    // without anyone pressing refresh.
    invoke("sessions_list").then(renderSessions).catch(() => {});
  }
}

function setRevisePlaceholders() {
  $("prompt").placeholder = "Want it different? Say what to change…";
  $("composerHint").textContent = "changes edit the app in place · a few minutes, the AI reads before it edits";
}

function failBuild(why, request) {
  // Settle once. If the build already succeeded, ignore this later failure --
  // most importantly a watchdog tick that fires just after create resolved.
  if (state.buildSettled) return;
  state.buildSettled = true;
  clearInterval(state.watchdog);
  settleChipBad(state.buildChip, state.buildVersion || 1, () => make(request));
  state.buildChip = null;
  clearInterval(state.timer);
  state.lastFailed = request;
  const built = state.buildingSession || state.session;
  sayTo(built, "KRATE", why === "stopped" ? "stopped" : "that build didn't come together");
  persistSession(built);
  if (!(state.session && built && state.session.id === built.id)) return;
  /* The one hard rule of this card: plain words. A person here must never
   * meet a compiler error, an exit code, or a crate name. */
  if (why === "stopped") {
    $("failTitle").textContent = "Stopped.";
    $("failWhy").textContent = "Nothing was lost -- your words are kept, ready to send again.";
  } else {
    $("failTitle").textContent = "That one didn't come together.";
    $("failWhy").textContent = why;
    // The raw engine tail rides under the plain-words line. Two screenshots
    // in a row said "needs signing in" for a PATH problem and a toolchain
    // problem; a failure screen that hides its evidence costs a debugging
    // round trip per bug.
    $("failRaw").textContent = String(state.lastError || "").slice(-400);
  }
  show("failed");
}

function friendlyAsk(cap) {
  const map = {
    "ui.window:create": "open a window",
    "io.stdout": "print text",
    "io.args": "read its start-up options",
    "store.kv": "save your data on this computer",
    "store.sql": "keep records on this computer",
    "time.clock": "read the clock",
    "net.http": "reach the internet",
    "fs.read": "read files you choose",
    "fs.write": "save files you choose",
  };
  return map[cap] || map[cap.split(":")[0]] || cap;
}

/* ---- driving the engine ----------------------------------------------- */

async function make(request) {
  invoke("dbg_log", { line: `make() request=${JSON.stringify(request).slice(0,40)} buildingSession=${state.buildingSession?.id||"null"} session=${state.session?.id||"null"} hasResult=${!!currentApp()}` }).catch(()=>{});
  // Two builds at once would leave the first unstoppable; the backend
  // refuses it too, but stopping here keeps the UI honest. Keyed on the
  // building session, not the visible phase -- browsing away changes the
  // phase while the build very much continues.
  if (state.buildingSession) { invoke("dbg_log", { line: "make() BAILED: buildingSession set" }).catch(()=>{}); return; }
  if (!state.session) newSession(request);
  // The stage belongs to what is happening NOW. Without this a retry from
  // a rail chip left the previous failure card on screen while the plan
  // step ran and the next build started -- the founder watched exactly
  // that after a timeout.
  if (state.phase === "failed" || state.phase === "done") show("idle");
  const files = state.attachments.slice();
  state.attachments = [];
  renderAttachments();
  say("YOU", request, files);
  persist();
  $("prompt").value = "";

  // "It won't open" is a problem report, not a change request. The first
  // real user said exactly that and watched the studio announce it was
  // "making that change" -- editing an app nobody can open. Check the app
  // ourselves and report what the runtime says instead.
  if (currentApp() && /\b(can'?t|cannot|unable|won'?t|doesn'?t|not)\s+(open|start|launch|run|work)|crash|nothing happens|not working|no window/i.test(request)) {
    return diagnoseCurrent();
  }
  // A change to an app that already works goes straight to the build: the
  // conversation already happened.
  if (currentApp()) return buildNow(request, files, true);
  // The conversation gate (K-123): before anything builds, the request is
  // looked at -- questions when answering them would change what gets
  // built, a plan otherwise. "Sadas" becomes a question, never an app.
  if (state.planning) return continuePlanning(request, files);
  return startPlanning(request, files);
}

/* ---- reporting an issue (K-128) --------------------------------------- */

/* The consent dialog names the real contents of the real file, gathered
   before it is shown -- a promise about what will be sent is not the same
   as a list of what IS in it, and only the second one earns a click. */
async function openReportSheet() {
  const session = state.session;
  if (!session) return;
  const sheet = $("reportSheet");
  $("repList").innerHTML = "<li class=\"dim\">Gathering this session…</li>";
  $("repSize").textContent = "";
  $("repResult").textContent = "";
  $("repNote").value = "";
  $("repSend").disabled = true;
  sheet.classList.remove("hidden");
  try {
    const info = await invoke("report_collect", { session: session.id });
    state.report = info;
    const WORDS = {
      "session.json": "this conversation, and what Krate said back",
      "about.txt": "your Krate version, operating system, and which AI tools are installed",
      "workspace/.agent-transcript.txt": "the AI's own log of what it did",
      "workspace/src/lib.rs": "the app code the AI had written",
      "workspace/Cargo.toml": "the app's build setup",
      "workspace/manifest.toml": "the permissions the app declared",
    };
    $("repList").innerHTML = "";
    for (const f of info.files) {
      const li = document.createElement("li");
      li.textContent = WORDS[f] || f;
      $("repList").appendChild(li);
    }
    $("repSize").textContent = `${Math.max(1, Math.round(info.size / 1024))} KB in total.`;
    $("repSend").disabled = false;
  } catch (err) {
    $("repList").innerHTML = "";
    $("repResult").textContent = plainWords(err);
  }
}

async function sendReport() {
  if (!state.report) return;
  $("repSend").disabled = true;
  $("repSend").textContent = "Sending…";
  try {
    const said = await invoke("report_send", {
      path: state.report.path,
      session: state.session ? state.session.id : "",
      note: $("repNote").value.trim(),
    });
    $("reportSheet").classList.add("hidden");
    say("KRATE", `Sent to Krate support (${said}). Thank you -- this is how the next person avoids it.`, null, { variant: "ask" });
  } catch (err) {
    $("repResult").textContent = plainWords(err);
    $("repSend").disabled = false;
  } finally {
    $("repSend").textContent = "Send to support";
  }
}

/* When someone says the app will not open, run it and read the answer. */
async function diagnoseCurrent() {
  const app = currentApp();
  say("KRATE", "Let me try opening it myself and see what happens…");
  try {
    const verdict = await invoke("diagnose_app", { path: app.path });
    if (verdict === "ok") {
      say("KRATE", "It starts and draws its first screen when I run it here, so the app itself is healthy. Try updating Krate (the Update chip at the top if one is showing), then open it again. If it still won't open on a double-click, tell me what you see and I'll dig further.", null, { variant: "ask" });
    } else {
      say("KRATE", `Found it -- when I run the app, this happens:\n\n${verdict}\n\nTell me to fix it and I'll make that change.`, null, {
        variant: "ask",
        actions: [{ label: "Fix it", primary: true, run: () => make(`The app fails to start. When run, it reports:\n${verdict}\nFix that.`) }],
      });
    }
  } catch (err) {
    say("KRATE", `I couldn't check it (${plainWords(err)}).`);
  }
  persist();
}

/* ---- the conversation before the build (K-123) ------------------------- */

function planContext() {
  const p = state.planning;
  let text = p.request;
  for (const qa of p.qa) {
    text += `\n\n(When asked "${qa.q}" the person answered: "${qa.a}")`;
  }
  return text;
}

async function startPlanning(request, files) {
  state.planning = { request, files, qa: [], rounds: 0, lastQuestions: [] };
  // Speak IMMEDIATELY. The plan call can take ten seconds, and ten silent
  // seconds after a person's very first message reads as broken.
  say("KRATE", "Looking at your request…");
  setIdleNote("Reading your request…");
  await runPlan();
}

async function continuePlanning(text, files) {
  state.planning.files.push(...files);
  // The escape hatch, always available: the person is the boss.
  if (/^\s*(just\s+)?(build|make|go|start|yes|ok|okay|do|sure|yep|yeah)\s*(it|now|that|this)?\s*[.!]*\s*$/i.test(text)) {
    return finishPlanningAndBuild();
  }
  state.planning.qa.push({
    q: state.planning.planShown
      ? "anything to change about the plan?"
      : state.planning.lastQuestions.join(" / ") || "anything to add?",
    a: text,
  });
  // A plan was already shown: whatever they just said is the final word --
  // agreement, a tweak, anything -- and it builds WITH those words folded
  // in. The first live session answered a plan with "thousands of
  // particles yes" and got a second, longer plan; a plan is a preview,
  // never a negotiation loop.
  if (state.planning.planShown) {
    return finishPlanningAndBuild();
  }
  await runPlan();
}

/* Is the agent we are about to run actually usable?
 *
 * Cheap insurance against sending work to an agent that cannot take it. The
 * failure this prevents took one second and named `claude` on a machine whose
 * chip said Codex -- the worst kind of error, because the thing it blames is
 * not the thing the user chose. If the roster says our agent is not working,
 * re-probe once (it may have been installed since) and switch to one that is
 * before starting, rather than after failing. */
async function ensureUsableAgent() {
  const usable = (name) =>
    (state.agents || []).some((a) => a.name === name && a.state === "working");
  if (usable(state.agent)) return true;
  await refreshAgents();
  return usable(state.agent);
}

async function runPlan() {
  $("composerHint").textContent = "thinking it through…";
  $("send").disabled = true;
  await ensureUsableAgent();
  try {
    const raw = await invoke("plan_request", {
      request: planContext(),
      attachments: state.planning.files,
      agent: state.agent,
    });
    const answer = JSON.parse(raw);
    if (answer.ask && answer.ask.length && state.planning.rounds < 1) {
      // ONE round of questions, ever. The first live session got two
      // rounds and called it what it is: frustrating.
      state.planning.rounds += 1;
      state.planning.lastQuestions = answer.ask;
      const questions = answer.ask.map((q, i) => `${i + 1}. ${q}`).join("\n");
      say("KRATE", questions, null, {
        variant: "ask",
        actions: [{ label: "Skip and build", run: finishPlanningAndBuild }],
      });
      setIdleNote("Answer on the left and I'll start building.");
      $("prompt").placeholder = "Answer here…";
    } else if (answer.plan) {
      state.planning.plan = answer.plan;
      state.planning.planShown = true;
      const needs = (answer.needs || []).filter(Boolean);
      const needsLine = needs.length
        ? `\n\nFrom you it needs: ${needs.join("; ")}.`
        : "";
      say("KRATE", `Here's what I'll build: ${answer.plan}${needsLine}`, null, {
        actions: [
          { label: "Build it", primary: true, run: finishPlanningAndBuild },
        ],
      });
      $("prompt").placeholder = "Anything to change? Your next message starts the build";
      setIdleNote("The plan is on the left. Say build it and I'll start.");
    } else {
      return finishPlanningAndBuild();
    }
  } catch (err) {
    // The conversation must never become a wall in front of building --
    // and its failure must never read like one. One plain line, no nested
    // error prose (the first live run printed a build error inside a
    // parenthesis inside an apology).
    console.warn("plan step failed:", err);
    say("KRATE", "I'll skip the questions this time and build right away.");
    return finishPlanningAndBuild();
  } finally {
    $("composerHint").textContent = "";
    $("send").disabled = false;
  }
  persist();
}

/* The idle stage's one line. It is the only thing on the right while the
   conversation happens, so it should say what is going on rather than
   showing the last build's ghost. */
function setIdleNote(text) {
  const note = $("idleNote");
  if (note) note.textContent = text;
}

function finishPlanningAndBuild() {
  const p = state.planning;
  state.planning = null;
  $("prompt").placeholder = "Describe the app you want…";
  let enriched = p.request;
  for (const qa of p.qa) {
    enriched += `\n\n(When asked "${qa.q}" the person answered: "${qa.a}")`;
  }
  if (p.plan) {
    enriched += `\n\n(The agreed plan: ${p.plan})`;
  }
  return buildNow(enriched, p.files, false);
}

async function buildNow(request, files, revising) {
  invoke("dbg_log", { line: `buildNow() revising=${revising} buildingSession=${state.buildingSession?.id||"null"} session=${state.session?.id||"null"}` }).catch(()=>{});
  if (state.buildingSession) { invoke("dbg_log", { line: "buildNow() BAILED: buildingSession set" }).catch(()=>{}); return; }
  // The composer stays live during a build so a thought can be queued
  // rather than lost.
  $("prompt").placeholder = "Add a change - it runs when this finishes…";

  state.buildingSession = state.session;
  state.lastRequest = request;
  renderBuilding();
  startBuildWatchdog();
  // The rail is a conversation: it should answer. Without this the left
  // side showed one line and then nothing for six minutes while the right
  // side did all the talking.
  const version = (state.session.builds || 0) + 1;
  say("KRATE", revising
    ? "Reading your app, then making that change."
    : "On it. I'll show you each step as it happens.");
  // Warn BEFORE the first flash, not after. While it works, the AI opens
  // the app to look at it and fix what it sees -- windows appear for a
  // second and sounds play. Unexplained, that reads as the machine
  // misbehaving; the founder watched exactly that (K-132).
  say("KRATE", "While I work, I'll open your app a few times to look at it -- so a window may flash and you might hear its sounds. That's me testing it, not something breaking.");
  // The build itself is one chip on the timeline, born live and settled
  // into a receipt when it ends -- narration never piles up in the rail.
  state.buildChip = appendLiveChip(version);
  state.buildVersion = version;
  // A build settles exactly once. The watchdog fires on its own interval and
  // the create promise resolves on its own; both call in to settle the build,
  // and without a guard a watchdog that fires a moment before create resolves
  // settles the chip to "failed" AND then finishBuild fills the done card --
  // the left panel says failed while the right shows a completed app. The
  // founder watched exactly this. First outcome wins; the later one is ignored.
  state.buildSettled = false;
  beginBuild(
    revising ? "Making your change" : "Making your app",
    revising ? "the AI reads your app before it edits" : "usually a few minutes",
  );

  try {
    const result = revising
      ? await invoke("revise_app", {
          path: currentApp().path,
          change: request,
          agent: state.agent,
          attachments: files,
        })
      : await invoke("create_app", {
          request,
          agent: state.agent,
          attachments: files,
          outDir: state.outDir,
          session: state.session.id,
        });
    finishBuild(result);
  } catch (err) {
    state.lastError = String(err);
    const text = String(err);
    // A feasibility refusal is an ANSWER, not an error: the AI read the
    // request and Krate's API and concluded the app cannot truthfully work
    // (a fake on-screen keyboard was the live case: apps cannot send
    // keystrokes to other programs, by design). "Try again" would be a
    // lie; say what is true and invite a different idea.
    const refusal = text.match(/Krate cannot build that: ([^]*?)(?:\n\n|$)/);
    if (refusal) {
      say("KRATE", `This one can't work as a real app: ${refusal[1].trim()}\n\nTell me a different version of the idea and I'll build that.`, null, { variant: "ask" });
      show("idle");
    } else {
      failBuild(plainWords(err), request);
    }
  } finally {
    state.buildingSession = null;
    renderBuilding();
    $("send").disabled = false;
    const queued = state.queued;
    state.queued = null;
    if (queued) {
      setRevisePlaceholders();
      setTimeout(() => make(queued), 400);
    }
  }
}

/* Sign-in failures, in sign-in words.
 *
 * These used to run through plainWords(), the BUILD error translator, so a
 * failed login said "Something in the build went wrong" on a screen with no
 * build on it. A person reading that has no idea what to do, and clicking
 * again does nothing -- which is exactly what happened to the first people
 * who tried it. */
function signInWords(err) {
  const text = String(err && err.message ? err.message : err || "");
  if (/unexpected argument|unrecognized|usage:/i.test(text))
    return "This copy of Krate Studio and the Krate engine disagree. Update both from krate.tech.";
  if (/could not run|could not start|No such file/i.test(text))
    return "The Krate engine is missing from this install. Reinstall Krate Studio from krate.tech.";
  if (/network|offline|dns|connect|timed out|resolve/i.test(text))
    return "GitHub could not be reached. Check your connection and try again.";
  if (/denied|expired|declined/i.test(text))
    return "The sign-in was not approved in time. Press the button to get a new code.";
  // Anything else: show what actually happened rather than inventing a
  // reason. An unfamiliar error a person can read out is worth more than a
  // reassuring sentence that is wrong.
  return text.trim() ? `Sign-in failed: ${text.trim()}` : "Sign-in failed. Try again.";
}

function plainWords(err) {
  const text = String(err && err.message ? err.message : err);
  if (text === "stopped") return "stopped";
  // A broken install must say so. Falling through to the generic build
  // message told people to "try again" when nothing could ever work.
  if (/could not run the Krate engine|could not start the Krate engine|KRATE_STUDIO_ENGINE/i.test(text))
    return "Krate's engine is missing from this install. Reinstall Krate from krate.tech.";
  if (/no such file|not found|No such file or directory/i.test(text) && /krate/i.test(text))
    return "Krate's engine is missing from this install. Reinstall Krate from krate.tech.";
  if (/is not there any more/i.test(text)) return text;
  if (/already being made/i.test(text)) return text;
  if (/no AI|not installed|unknown AI provider/i.test(text))
    return "No AI is connected yet. Open the AI menu at the top to set one up.";
  // Specific before general: a rustup install log mentions authentication
  // incidentally, and matching the sign-in guess first told a person with a
  // PATH problem to go sign in -- a wrong door with a confident sign on it.
  if (/toolchain|rustup|cargo/i.test(text)) return "The build tools aren't set up yet. Trying again lets Krate install them.";
  if (/quota|rate.?limit/i.test(text)) return "Your AI is out of quota right now. It usually comes back within the hour.";
  // The AI's own sandbox broke: its words, not a guess. Seen live with
  // Codex on Windows (its sandbox helper missing), where every command the
  // agent ran failed and the card blamed sign-in instead (K-124).
  if (/sandbox.*(helper|launch_failed)|orchestrator_helper/i.test(text))
    return "Your AI's own sandbox is broken on this machine. Reinstall that AI, or pick another one from its menu at the top.";
  // \bauth catches authentication/unauthorized; the (?!or) guard keeps
  // "author command failed" -- our own generic failure line -- from telling
  // every user to go sign in (K-124: it did exactly that).
  if (/sign ?in|\bauth(?!or)|logged/i.test(text)) return "Your AI needs signing in. Click its name at the top for the fix.";
  if (/network|offline|dns|connect/i.test(text)) return "The internet connection dropped mid-build.";
  return "Something in the build went wrong. Trying again usually works; your words are kept.";
}

/* ---- agents ----------------------------------------------------------- */

async function refreshAgents() {
  try {
    state.agents = await invoke("agents");
    state.agentsError = null;
  } catch (err) {
    // The full error rides on the chip's tooltip AND into the AI sheet, so
    // "engine not found" on someone else's machine is diagnosable from a
    // screenshot instead of a guessing round.
    state.agentsError = String(err);
    setChips("bad", "engine trouble", String(err));
    return;
  }
  const chosen =
    state.agents.find((a) => a.name === state.agent && a.state === "working") ||
    state.agents.find((a) => a.state === "working") ||
    state.agents[0];
  if (!chosen) {
    setChips("bad", "no AI found", "");
    return;
  }
  // The chip shows `chosen`, so the build must USE `chosen`. Updating
  // state.agent only in the "working" case let the two disagree: the chip
  // named the agent that was found while the build still ran the one that was
  // configured, and the mismatch surfaced as an instant failure naming an
  // agent the user never picked. If it is worth painting, it is what runs.
  state.agent = chosen.name;
  const dot = chosen.state === "working" ? "ok" : chosen.state === "not-ready" ? "warn" : "bad";
  const text =
    chosen.state === "working" ? chosen.label
    : chosen.state === "not-ready" ? `${chosen.label} · needs a fix`
    : `${chosen.label} · not installed`;
  setChips(dot, text, chosen.detail || "");
}

function setChips(dot, text, title) {
  for (const suffix of ["", "2"]) {
    const d = $(`agentDot${suffix}`);
    const n = $(`agentName${suffix}`);
    if (d) d.className = `dot ${dot}`;
    if (n) n.textContent = text;
    const chip = $(`agentChip${suffix}`);
    if (chip) chip.title = title || "";
  }
  // The home bar's "Built by" chip carries the same truth.
  const bbn = $("builtByName");
  const bbd = $("builtByDot");
  if (bbn) {
    const chosen = state.agents.find((a) => a.name === state.agent);
    bbn.textContent = chosen ? chosen.label : state.agent;
  }
  if (bbd) bbd.className = `dot ${dot}`;
}

function openAiSheet() {
  const list = $("aiList");
  list.innerHTML = "";
  if (state.agentsError && !state.agents.length) {
    const p = document.createElement("p");
    p.className = "ai-error";
    p.textContent = state.agentsError;
    list.appendChild(p);
  }
  for (const a of state.agents) {
    const row = document.createElement("div");
    row.className = "ai-row";
    const dot = a.state === "working" ? "ok" : a.state === "not-ready" ? "warn" : "bad";
    const detail =
      a.state === "working" ? (a.name === state.agent ? "ready · in use" : "ready")
      : a.detail || (a.state === "missing" ? "not installed" : "not ready");
    row.innerHTML = `
      <span class="dot ${dot}"></span>
      <div class="grow">
        <p class="ai-name"></p>
        <p class="ai-detail"></p>
        ${a.remedy ? `<p class="ai-remedy"></p>` : ""}
      </div>`;
    row.querySelector(".ai-name").textContent = a.label;
    row.querySelector(".ai-detail").textContent = detail;
    if (a.remedy) row.querySelector(".ai-remedy").textContent = a.remedy;
    if (a.state === "working" && a.name !== state.agent) {
      const use = document.createElement("button");
      use.className = "btn";
      use.textContent = "Use this";
      use.addEventListener("click", async () => {
        state.agent = a.name;
        await invoke("settings_set", { settings: { out_dir: state.outDir, agent: state.agent } });
        refreshAgents();
        $("aiSheet").classList.add("hidden");
      });
      row.appendChild(use);
    } else if (a.state === "missing" && a.install_package) {
      // Installing here, rather than printing a command. Someone making their
      // first app should never have to find a terminal, leave this window,
      // and come back.
      const add = document.createElement("button");
      add.className = "btn btn-primary";
      add.textContent = "Install";
      add.addEventListener("click", async () => {
        add.disabled = true;
        add.textContent = "Installing…";
        const note = row.querySelector(".ai-detail");
        note.classList.add("installing");
        try {
          await invoke("install_agent", { name: a.name });
          note.classList.remove("installing");
          note.textContent = "installed -- sign in to it once to finish";
          add.textContent = "Done";
          refreshAgents();
        } catch (err) {
          note.classList.remove("installing");
          note.textContent = String(err);
          add.disabled = false;
          add.textContent = "Install";
        }
      });
      row.appendChild(add);
    }
    list.appendChild(row);
  }
  $("aiSheet").classList.remove("hidden");
}

/* The gate watches for a sign-in completed elsewhere.
 *
 * On Windows and Linux the browser hand-off lands in a SECOND studio
 * process that stores the identity and exits; the window the person left
 * open has no event to hear. So while the gate is showing, ask the engine
 * every few seconds -- and immediately on focus, which is the moment they
 * switch back from the browser. */
async function refreshAccountAndEnter() {
  try {
    const account = await invoke("account_status");
    if (account && account.signed_in) {
      state.account = account;
      enterHome();
      return true;
    }
  } catch {}
  return false;
}
setInterval(() => {
  if (!$("viewGate").classList.contains("hidden")) refreshAccountAndEnter();
}, 3000);
window.addEventListener("focus", () => {
  if (!$("viewGate").classList.contains("hidden")) refreshAccountAndEnter();
});

/* Re-check the AIs when the window comes back.
 *
 * Someone who signs in to their AI, or installs one, in another window should
 * not have to restart the studio to be noticed -- "not installed" while it is
 * plainly installed is the single most confusing thing this app can say.
 * Re-checking on focus makes the fix take effect the moment they come back,
 * with no button to find.
 *
 * Throttled: the probe actually runs each tool, so doing it on every focus
 * event would spawn processes every time the window is touched. */
let lastAgentCheck = 0;
window.addEventListener("focus", () => {
  const now = Date.now();
  if (now - lastAgentCheck < 5000) return;
  lastAgentCheck = now;
  refreshAgents();
});

/* ---- attachments ------------------------------------------------------ */

async function attach() {
  const picked = await invoke("pick_files");
  for (const p of picked) {
    if (!state.attachments.includes(p)) state.attachments.push(p);
  }
  renderAttachments();
}

/* ---- app details ------------------------------------------------------- */

/* A capability string is precise and unreadable. These are the same facts in
   words, so a person can decide whether an app should be doing that. Anything
   without a phrasing shows its raw name rather than being hidden -- an
   unexplained permission is exactly the one worth seeing. */
const CAP_WORDS = {
  "ui.window:create": "Open a window",
  "ui.dialog:confirm": "Ask you yes-or-no questions",
  "ui.dialog:message": "Show you messages",
  "ui.dialog:file-open": "Ask you to pick a file to open",
  "ui.dialog:file-save": "Ask you where to save a file",
  "ui.dialog:open-folder": "Ask you to pick a folder",
  "gfx.gpu:basic": "Draw with the graphics card",
  "io.stdout": "Print text",
  "io.stderr": "Print errors",
  "io.stdin": "Read typed input",
  "io.args": "Read the options it was started with",
  "io.log": "Write to its own log",
  "time.clock": "Read the current time",
  "time.monotonic": "Measure how long things take",
  "time.sleep": "Wait",
  "locale.info": "Know your language and region",
  "locale.format": "Format numbers and dates the way you write them",
  "random.bytes": "Use random numbers",
  "net.http": "Reach the internet",
  "store.kv": "Save its own settings",
  "store.sql": "Keep its own database",
  "clipboard.read": "Read the clipboard",
  "clipboard.write": "Put things on the clipboard",
  "audio.playback": "Play sound",
  "audio.capture": "Record from the microphone",
};

async function showInfo() {
  const app = currentApp();
  if (!app) return;
  const sheet = $("infoSheet");
  $("infoName").textContent = app.name || "Your app";
  $("infoWhere").textContent = "Reading the app…";
  $("infoRows").innerHTML = "";
  $("infoCaps").innerHTML = "";
  sheet.classList.remove("hidden");

  try {
    const info = await invoke("app_info", { path: app.path });
    $("infoWhere").textContent = info.path;

    const rows = [
      ["Size", `${Math.round((info.size || 0) / 1024)} KB`],
      ["Runs on", "Mac, Windows and Linux"],
      // The content hash IS the app's identity: the same bytes anywhere are
      // the same app, which is what makes a share link trustworthy.
      ["Fingerprint", (info.identity || "").slice(0, 16) || "unknown"],
    ];
    for (const [k, v] of rows) {
      const dt = document.createElement("dt");
      dt.textContent = k;
      const dd = document.createElement("dd");
      dd.textContent = v;
      $("infoRows").append(dt, dd);
    }

    for (const cap of info.capabilities || []) {
      const li = document.createElement("li");
      li.textContent = CAP_WORDS[cap] || cap;
      li.title = cap;
      $("infoCaps").appendChild(li);
    }
    if (!(info.capabilities || []).length) {
      const li = document.createElement("li");
      li.textContent = "Nothing beyond drawing its own window.";
      $("infoCaps").appendChild(li);
    }
  } catch (err) {
    $("infoWhere").textContent = String(err);
  }
}

/* ---- Krate Cloud ------------------------------------------------------- */

function timeAgo(seconds) {
  const mins = Math.max(1, Math.round((Date.now() / 1000 - seconds) / 60));
  if (mins < 60) return `${mins} min ago`;
  const hours = Math.round(mins / 60);
  if (hours < 24) return `${hours} h ago`;
  const days = Math.round(hours / 24);
  return days === 1 ? "yesterday" : `${days} days ago`;
}

/* The shelf an app sits on comes from the hub -- the publisher classified it
   at publish time. The keyword fallback only covers apps published before the
   hub stored a category. */
const CLOUD_CATS = [
  { id: "all", label: "Everything" },
  { id: "games", label: "Games" },
  { id: "productivity", label: "Productivity" },
  { id: "tools", label: "Tools" },
  { id: "media", label: "Media" },
  { id: "learning", label: "Learning" },
  { id: "apps", label: "More" },
];

function catOf(app) {
  const meta = app.meta || {};
  if (meta.category) return [meta.category];
  const hay = `${meta.name || ""} ${meta.description || ""}`.toLowerCase();
  if (["game", "dash", "nova", "flip", "dice", "arcade"].some((w) => hay.includes(w))) return ["games"];
  if (["note", "journal", "timer", "clock", "focus", "list", "track"].some((w) => hay.includes(w))) return ["productivity"];
  if (["calc", "split", "convert", "rename", "budget", "tip"].some((w) => hay.includes(w))) return ["tools"];
  return ["apps"];
}

function catLabel(id) {
  const cat = CLOUD_CATS.find((c) => c.id === id);
  return cat ? cat.label : id;
}

/* Apps have no uploaded logos, so each gets a monogram tile whose colour is
   derived from its own name -- stable across sessions, distinct across apps. */
function appTile(name) {
  let h = 0;
  for (const ch of name) h = (h * 31 + ch.codePointAt(0)) % 360;
  const tile = document.createElement("span");
  tile.className = "cloud-icon";
  tile.style.background =
    `linear-gradient(135deg, hsl(${h} 42% 30%), hsl(${(h + 40) % 360} 46% 18%))`;
  const words = name.trim().split(/\s+/);
  tile.textContent = words.length > 1
    ? (words[0][0] + words[1][0]).toUpperCase()
    : name.slice(0, 2).replace(/^./, (c) => c.toUpperCase());
  return tile;
}

async function openCloud() {
  showView("cloud");
  $("cloudError").classList.add("hidden");
  $("cloudLoading").classList.remove("hidden");
  $("cloudGrid").innerHTML = "";
  $("cloudCount").textContent = "";
  try {
    const payload = JSON.parse(await invoke("cloud_apps"));
    state.cloud = (payload.apps || []).map((app) => ({ ...app, cats: catOf(app) }));
    renderCloudCats();
    filterCloud();
  } catch (err) {
    $("cloudError").textContent = String(err);
    $("cloudError").classList.remove("hidden");
  } finally {
    $("cloudLoading").classList.add("hidden");
  }
}

function renderCloudCats() {
  const box = $("cloudCats");
  box.innerHTML = "";
  for (const cat of CLOUD_CATS) {
    // A category nobody has published to is noise, so it is not drawn.
    const count = cat.id === "all"
      ? state.cloud.length
      : state.cloud.filter((a) => a.cats.includes(cat.id)).length;
    if (!count) continue;
    const chip = document.createElement("button");
    chip.className = "cat" + (state.cloudCat === cat.id ? " on" : "");
    chip.textContent = cat.label;
    chip.addEventListener("click", () => {
      state.cloudCat = cat.id;
      renderCloudCats();
      filterCloud();
    });
    box.appendChild(chip);
  }
}

/* One published app, in full: its screenshot, who made it, what it is allowed
   to do -- and only then a button that runs it. Browsing a gallery should not
   be one click away from executing something. */
function showCloudApp(app) {
  const meta = app.meta || {};
  state.cloudApp = app;
  showView("appDetail");
  $("appCrumb").textContent = meta.name || "App";
  const head = $("detailHead");
  head.querySelector(".cloud-icon")?.remove();
  if (app.icon) {
    const icon = document.createElement("img");
    icon.className = "cloud-icon";
    icon.src = app.icon;
    icon.alt = "";
    icon.onerror = () => icon.replaceWith(appTile(meta.name || "App"));
    head.prepend(icon);
  } else {
    head.prepend(appTile(meta.name || "App"));
  }
  $("detailName").textContent = meta.name || "Untitled app";
  $("detailTag").textContent = catLabel((app.cats && app.cats[0]) || "apps");
  $("detailDesc").textContent = meta.description || "";
  $("detailDesc").classList.toggle("hidden", !meta.description);
  $("detailBy").textContent = meta.author ? `Made by ${meta.author}` : "";
  $("detailNote").textContent = "";

  const shot = $("detailShot");
  shot.innerHTML = "";
  if (app.shot) {
    const img = document.createElement("img");
    img.src = app.shot;
    img.alt = `${meta.name || "The app"}, as it renders`;
    // The hub has no screenshot for every app; a broken image frame looks
    // like a fault, so it removes itself and the placeholder shows instead.
    img.onerror = () => { shot.innerHTML = ""; shot.appendChild(shotPlaceholder()); };
    shot.appendChild(img);
  } else {
    shot.appendChild(shotPlaceholder());
  }

  const rows = [
    ["Size", `${Math.round((meta.size || 0) / 1024)} KB`],
    ["Published", meta.published ? timeAgo(meta.published) : "unknown"],
    ["Runs on", "Mac, Windows and Linux"],
  ];
  $("detailRows").innerHTML = "";
  for (const [k, v] of rows) {
    const dt = document.createElement("dt");
    dt.textContent = k;
    const dd = document.createElement("dd");
    dd.textContent = v;
    $("detailRows").append(dt, dd);
  }

  // Permissions come from the engine reading the published bundle, without
  // running it -- the same wall that will apply if it is opened.
  const caps = $("detailCaps");
  caps.innerHTML = '<li class="dim">Checking what it can do…</li>';
  invoke("app_info", { path: app.url })
    .then((info) => {
      caps.innerHTML = "";
      const list = info.capabilities || [];
      if (!list.length) {
        const li = document.createElement("li");
        li.textContent = "Nothing beyond drawing its own window.";
        caps.appendChild(li);
        return;
      }
      for (const cap of list) {
        const li = document.createElement("li");
        li.textContent = CAP_WORDS[cap] || cap;
        li.title = cap;
        caps.appendChild(li);
      }
    })
    .catch(() => {
      caps.innerHTML = '<li class="dim">Could not read this app right now.</li>';
    });
}

function shotPlaceholder() {
  const box = document.createElement("div");
  box.className = "shot-none";
  box.textContent = "No screenshot yet";
  return box;
}

function filterCloud() {
  const q = ($("cloudSearch").value || "").trim().toLowerCase();
  const shown = state.cloud.filter((app) => {
    const meta = app.meta || {};
    const inCat = state.cloudCat === "all" || app.cats.includes(state.cloudCat);
    const hay = `${meta.name || ""} ${meta.description || ""} ${meta.author || ""}`.toLowerCase();
    return inCat && (!q || hay.includes(q));
  });
  $("cloudCount").textContent = shown.length
    ? `${shown.length} app${shown.length === 1 ? "" : "s"}`
    : "";
  renderCloud(shown, Boolean(q) || state.cloudCat !== "all");
}

function renderCloud(apps, filtered) {
  const grid = $("cloudGrid");
  grid.innerHTML = "";
  $("cloudError").classList.add("hidden");
  if (!apps.length) {
    $("cloudError").textContent = filtered
      ? "Nothing here matches that."
      : "Nothing published yet. Yours could be first.";
    $("cloudError").classList.remove("hidden");
    return;
  }
  for (const app of apps) {
    const meta = app.meta || {};
    const card = document.createElement("button");
    card.className = "cloud-card";

    // The app's own first frame on top -- a store where you can SEE the
    // apps reads as a store; a list of names reads as a mess.
    const shot = document.createElement("div");
    shot.className = "cloud-shot";
    if (app.shot) {
      const img = document.createElement("img");
      img.src = app.shot;
      img.loading = "lazy";
      img.alt = "";
      img.onerror = () => { img.remove(); };
      shot.appendChild(img);
    }
    card.appendChild(shot);

    // Icon tile, name and shelf on one line, like any app store row.
    const head = document.createElement("div");
    head.className = "cloud-id";
    if (app.icon) {
      const icon = document.createElement("img");
      icon.className = "cloud-icon";
      icon.src = app.icon;
      icon.alt = "";
      icon.onerror = () => icon.replaceWith(appTile(meta.name || "App"));
      head.appendChild(icon);
    } else {
      head.appendChild(appTile(meta.name || "App"));
    }
    const titles = document.createElement("div");
    titles.className = "cloud-titles";
    const name = document.createElement("p");
    name.className = "cloud-name";
    name.textContent = meta.name || "Untitled app";
    titles.appendChild(name);
    const tag = document.createElement("p");
    tag.className = "cloud-tag";
    tag.textContent = catLabel((app.cats && app.cats[0]) || "apps");
    titles.appendChild(tag);
    head.appendChild(titles);
    card.appendChild(head);

    if (meta.description) {
      const desc = document.createElement("p");
      desc.className = "cloud-desc";
      desc.textContent = meta.description;
      card.appendChild(desc);
    }

    const foot = document.createElement("div");
    foot.className = "cloud-foot";
    if (meta.author) {
      const by = document.createElement("p");
      by.className = "cloud-by";
      // The avatar is remote; a broken image would look like a bug, so it
      // simply removes itself.
      if (meta.avatar_url) {
        const img = document.createElement("img");
        img.src = meta.avatar_url;
        img.alt = "";
        img.onerror = () => img.remove();
        by.appendChild(img);
      }
      by.appendChild(document.createTextNode(meta.author));
      foot.appendChild(by);
    }
    const bits = [];
    if (meta.size) bits.push(`${Math.round(meta.size / 1024)} KB`);
    if (meta.published) bits.push(timeAgo(meta.published));
    if (bits.length) {
      const line = document.createElement("p");
      line.className = "cloud-meta";
      line.textContent = bits.join(" \u00b7 ");
      foot.appendChild(line);
    }
    card.appendChild(foot);

    card.addEventListener("click", () => showCloudApp(app));
    grid.appendChild(card);
  }
}

function renderAttachments() {
  for (const id of ["attachRow", "homeAttachRow"]) {
    paintAttachRow($(id));
  }
}

function paintAttachRow(row) {
  if (!row) return;
  row.classList.toggle("hidden", state.attachments.length === 0);
  row.innerHTML = "";
  state.attachments.forEach((p, i) => {
    const chip = document.createElement("span");
    chip.className = "attach-chip";
    const name = baseName(p);
    const ext = (name.match(/\.([a-z0-9]+)$/i) || [, "file"])[1];
    const kind = document.createElement("span");
    kind.className = "kind";
    kind.textContent = ext.slice(0, 4);
    chip.appendChild(kind);
    chip.appendChild(document.createTextNode(name));
    const x = document.createElement("button");
    x.textContent = "×";
    x.title = "Remove";
    x.addEventListener("click", () => {
      state.attachments.splice(i, 1);
      renderAttachments();
    });
    chip.appendChild(x);
    row.appendChild(chip);
  });
}

/* ---- done-card actions ------------------------------------------------ */

/// The finished app of the open session, or null.
///
/// Guards every action that needs one. A session can be open with no result
/// (a build that failed or was stopped), and reaching into `.path` there
/// throws -- which reads to a person as a button that silently does nothing.
function currentApp() {
  const r = state.session && state.session.result;
  return r && r.path ? r : null;
}

async function openApp() {
  const app = currentApp();
  if (!app) return;
  try {
    await invoke("open_app", { path: app.path });
  } catch (err) {
    showActionError(err);
  }
}

/// Say why an action on the finished app failed, where the person is
/// already looking -- the share row, which is the only line on this card
/// that can carry a sentence.
function showActionError(err) {
  const text = String(err && err.message ? err.message : err);
  $("shareLink").textContent = text;
  $("shareCopied").classList.add("hidden");
  $("shareResult").classList.remove("hidden");
  $("shareResult").classList.add("error");
}
/* The publish sheet: what the person is about to put in the store, shown
   before it goes -- name, one line, screenshot, optional logo. Publishing
   again later replaces the listing server-side, so this is also the edit
   path. */
const pubState = { shotPath: null, iconPath: null };

function openPublishSheet() {
  const app = currentApp();
  if (!app) return;
  pubState.shotPath = null;
  pubState.iconPath = null;
  $("pubName").value = (app.name || "").replace(/\.krate$/, "").replace(/-/g, " ")
    .replace(/\b\w/g, (c) => c.toUpperCase());
  const firstAsk = (state.session.messages.find((m) => m.who === "YOU") || {}).body || "";
  $("pubDesc").value = firstAsk.replace(/\s+/g, " ").trim().slice(0, 140);
  const shotImg = $("pubShotImg");
  if (app.shot) {
    shotImg.src = app.shot;
    shotImg.classList.remove("hidden");
    $("pubShotNone").classList.add("hidden");
  } else {
    shotImg.classList.add("hidden");
    $("pubShotNone").classList.remove("hidden");
  }
  $("pubIconImg").classList.add("hidden");
  $("pubIconNone").classList.remove("hidden");
  $("pubNote").textContent = "";
  $("pubGo").disabled = false;
  $("pubGo").textContent = "Publish";
  $("publishSheet").classList.remove("hidden");
}

async function pickPublishImage(kind) {
  const title = kind === "shot" ? "Choose a screenshot (PNG)" : "Choose a logo (PNG)";
  let path;
  try {
    path = await invoke("pick_image", { title });
  } catch (err) {
    $("pubNote").textContent = plainWords(err);
    return;
  }
  if (!path) return;
  try {
    const data = await invoke("read_image", { path });
    if (kind === "shot") {
      pubState.shotPath = path;
      $("pubShotImg").src = data;
      $("pubShotImg").classList.remove("hidden");
      $("pubShotNone").classList.add("hidden");
    } else {
      pubState.iconPath = path;
      $("pubIconImg").src = data;
      $("pubIconImg").classList.remove("hidden");
      $("pubIconNone").classList.add("hidden");
    }
    $("pubNote").textContent = "";
  } catch (err) {
    $("pubNote").textContent = plainWords(err);
  }
}

async function publishFromSheet() {
  const app = currentApp();
  if (!app) return;
  $("pubGo").disabled = true;
  $("pubGo").textContent = "Publishing…";
  $("pubNote").textContent = "";
  try {
    const url = await invoke("publish", {
      path: app.path,
      description: $("pubDesc").value.trim(),
      name: $("pubName").value.trim(),
      shot: pubState.shotPath,
      icon: pubState.iconPath,
    });
    state.session.result.share_url = url;
    $("publishSheet").classList.add("hidden");
    $("shareLink").textContent = url;
    $("shareResult").classList.remove("hidden", "error");
    let copied = false;
    try { await navigator.clipboard.writeText(url); copied = true; } catch (e) {}
    $("shareCopied").classList.toggle("hidden", !copied);
    persist();
  } catch (err) {
    $("pubNote").textContent = plainWords(err);
    $("pubGo").disabled = false;
    $("pubGo").textContent = "Publish";
  }
}

/* ---- settings --------------------------------------------------------- */

async function openSettings() {
  $("outDirValue").textContent = state.outDir;
  $("settingsSheet").classList.remove("hidden");
}

/* Two buttons that opened one sheet is two buttons too many. The account is
 * about who you are; settings is about where things go. */
function openAccount() {
  const a = state.account || {};
  $("accountName").textContent = a.name || a.login || "Signed in";
  $("accountLogin").textContent = a.login ? "@" + a.login : "";
  const img = $("accountAvatar");
  if (a.avatar_url) { img.src = a.avatar_url; img.classList.remove("hidden"); }
  else { img.classList.add("hidden"); }
  $("accountSheet").classList.remove("hidden");
}


/* ---- the rotating word ------------------------------------------------ */

/* krate.tech's own hero: the word swaps, letters leaving upward and
 * arriving from below with a blur, 40ms apart. Copied to the same numbers
 * so the app and the site read as one product. */
const ROT_WORDS = ["keep", "send", "run", "trust"];

function startRotator() {
  const el = $("rotWord");
  if (!el) return;
  if (matchMedia("(prefers-reduced-motion: reduce)").matches) return;
  let wi = 0;
  const setWord = (word, entering) => {
    el.innerHTML = "";
    word.split("").forEach((ch, i) => {
      const sp = document.createElement("span");
      sp.textContent = ch;
      sp.style.transition = "opacity .3s ease, transform .3s ease, filter .3s ease";
      sp.style.transitionDelay = i * 40 + "ms";
      if (entering) {
        sp.style.opacity = "0";
        sp.style.transform = "translateY(14px)";
        sp.style.filter = "blur(4px)";
        // A timeout, not a double rAF. The word sits inside a .reveal block
        // that starts hidden, and rAF callbacks for an offscreen/hidden
        // subtree do not reliably fire -- the letters stayed at opacity 0
        // with the entering styles applied and the word was invisible.
        // A timer always runs.
        setTimeout(() => {
          sp.style.opacity = "1";
          sp.style.transform = "none";
          sp.style.filter = "none";
        }, 20);
      }
      el.appendChild(sp);
    });
  };
  // The first word never animates in: it is already on screen when the
  // page reveals, and a first paint at opacity 0 is just an invisible
  // headline.
  setWord(ROT_WORDS[0], false);
  paintFlow(el);
  setInterval(() => {
    [...el.children].forEach((sp, i) => {
      sp.style.transitionDelay = i * 40 + "ms";
      sp.style.opacity = "0";
      sp.style.transform = "translateY(-14px)";
      sp.style.filter = "blur(4px)";
    });
    setTimeout(() => {
      wi = (wi + 1) % ROT_WORDS.length;
      setWord(ROT_WORDS[wi], true);
      paintFlow(el);
    }, 380);
  }, 3200);
}

/* Give every letter the same gradient, offset by where it sits in the word.
 *
 * A background clipped to text does not reach child elements, so each letter
 * must paint its own -- but if each paints a full colour ramp the word shows
 * one gradient PER CHARACTER, which is what "trust" looked like: five
 * separate runs instead of one flowing through it.
 *
 * Sizing the gradient to the whole word and shifting each letter's origin by
 * its own left offset makes the letters sample one continuous ramp. The tile
 * is two word-widths so the animation can travel exactly one tile and loop
 * with no seam.
 */
function paintFlow(word) {
  // Each letter is a window onto one word-wide gradient: measure where the
  // letter sits and how wide the word is, and hand both to the CSS as
  // custom properties. Measured from rects, so the enter/leave transforms
  // (translateY only) never skew the numbers. If the view is hidden the
  // rects are zero -- harmless, because every word change measures again.
  const base = word.getBoundingClientRect();
  if (!base.width) return;
  for (const sp of word.children) {
    const r = sp.getBoundingClientRect();
    sp.style.setProperty("--off", r.left - base.left + "px");
    sp.style.setProperty("--ww", Math.max(base.width, 60) + "px");
  }
}

/* ---- drag to resize the rail ------------------------------------------ */

/* How much room the conversation deserves against the app is a preference,
 * not a constant. Someone reading a long thread wants a wide rail; someone
 * watching a build wants a wide stage. The width persists. */
function setupDivider() {
  const divider = $("divider");
  const rail = document.querySelector(".rail");
  if (!divider || !rail) return;

  const saved = Number(localStorage.getItem("krate.railWidth") || 0);
  if (saved >= 240 && saved <= 720) {
    rail.style.width = rail.style.minWidth = saved + "px";
  }

  divider.addEventListener("mousedown", (e) => {
    e.preventDefault();
    document.body.classList.add("resizing");
    const move = (ev) => {
      // Bounded: a rail narrower than 240 cannot hold a sentence, and one
      // wider than 720 leaves no room for the app.
      const w = Math.max(240, Math.min(720, ev.clientX));
      rail.style.width = rail.style.minWidth = w + "px";
    };
    const up = () => {
      document.body.classList.remove("resizing");
      document.removeEventListener("mousemove", move);
      document.removeEventListener("mouseup", up);
      localStorage.setItem("krate.railWidth", parseInt(rail.style.width, 10) || 320);
    };
    document.addEventListener("mousemove", move);
    document.addEventListener("mouseup", up);
  });
}

/* ---- wiring ----------------------------------------------------------- */

function startFromHome() {
  const text = $("homePrompt").value.trim();
  if (!text) return;
  newSession(text);
  $("railTitle").textContent = state.session.title;
  $("thread").innerHTML = "";
  show("idle");
  showView("session");
  $("homePrompt").value = "";
  make(text);
}

function submitInSession() {
  const text = $("prompt").value.trim();
  if (!text) return;
  if (state.buildingSession) {
    if (state.session && state.buildingSession.id === state.session.id) {
      // Do not drop it. A thought that arrives mid-build is exactly the
      // thought worth keeping -- queue it, say so, and run it when this
      // build finishes.
      state.queued = text;
      $("prompt").value = "";
      say("YOU", text);
      say("KRATE", "Noted - I'll do that as soon as this one is finished.");
      $("composerHint").textContent = "queued · runs when this build finishes";
    } else {
      // A different session is building. Silence here would eat the words.
      say("KRATE", `"${state.buildingSession.title}" is still being made -- one app at a time. This will be ready to send once it finishes.`);
    }
    return;
  }
  make(text);
}

$("loginBtn").addEventListener("click", login);
$("homeSend").addEventListener("click", startFromHome);
// Enter sends -- the way every chat on earth works. Shift+Enter keeps the
// newline for anyone writing a longer brief.
$("homePrompt").addEventListener("keydown", (e) => {
  if (e.key === "Enter" && !e.shiftKey && !e.isComposing) { e.preventDefault(); startFromHome(); }
});

/* Both composers grow to fit what is typed, up to a cap. A one-line box
 * that scrolls internally is the single most common way a text field feels
 * cheap. */
function autoGrow(el) {
  el.style.height = "auto";
  el.style.height = Math.min(el.scrollHeight, 132) + "px";
}
["prompt", "homePrompt"].forEach((id) => {
  const el = $(id);
  if (el) el.addEventListener("input", () => autoGrow(el));
});

$("send").addEventListener("click", submitInSession);
$("prompt").addEventListener("keydown", (e) => {
  if (e.key === "Enter" && !e.shiftKey && !e.isComposing) { e.preventDefault(); submitInSession(); }
});
$("backBtn").addEventListener("click", async () => {
  await persist();
  enterHome();
});
$("loginBrowserBtn").addEventListener("click", async () => {
  $("gateError").classList.add("hidden");
  try {
    await invoke("login_browser");
    // The krate:// handoff emits the same login-step "done" the device flow
    // uses; nothing to poll here.
  } catch (err) {
    $("gateError").textContent = signInWords(err);
    $("gateError").classList.remove("hidden");
  }
});
$("attachBtn").addEventListener("click", attach);
$("homeAttachBtn").addEventListener("click", attach);
$("openKrateBtn").addEventListener("click", () => invoke("open_krate").catch(() => {}));
$("cloudBtn").addEventListener("click", openCloud);
$("cloudBackBtn").addEventListener("click", enterHome);
$("cloudRefresh").addEventListener("click", openCloud);
$("cloudSearch").addEventListener("input", filterCloud);
$("appBackBtn").addEventListener("click", () => showView("cloud"));
$("detailRun").addEventListener("click", async () => {
  const app = state.cloudApp;
  if (!app) return;
  const btn = $("detailRun");
  btn.disabled = true;
  btn.textContent = "Opening…";
  try {
    await invoke("cloud_run", { url: app.url });
    $("detailNote").textContent = "Opening -- it asks your permission before it can do anything.";
  } catch (err) {
    $("detailNote").textContent = String(err);
  }
  btn.disabled = false;
  btn.textContent = "Open it";
});
$("detailCopy").addEventListener("click", async () => {
  const app = state.cloudApp;
  if (!app) return;
  try {
    await navigator.clipboard.writeText(app.url);
    $("detailNote").textContent = "Link copied. Anyone with it can open this app.";
  } catch {
    $("detailNote").textContent = app.url;
  }
});
/* Stopping is a person's decision and must always be one click away: from
   the build stage, from the home bar, and from the live chip in the rail.
   All three call this. */
async function stopBuild() {
  // Stop must ALWAYS end the build in the UI, even when the engine is already
  // gone. The old version leaned entirely on the engine's exit landing in the
  // failure path to move the UI out of "building" -- but if the process had
  // already died (or never started; see K-136), that exit already happened and
  // was missed, so the clock ran forever and Stop looked broken. It was: the
  // founder clicked Stop on a build with no process and nothing changed.
  //
  // So kill whatever is there, then settle the build to "stopped" ourselves.
  // settleBuild is idempotent (buildSettled guards it), so if the engine's real
  // exit does still arrive it is a no-op rather than a double-settle.
  try {
    await invoke("stop_build");
  } catch (err) {
    // Even if the kill call errors, still end the build in the UI -- a Stop
    // that leaves a spinner running is worse than a Stop that logs an error.
    console.warn("stop_build failed:", err);
  }
  if (state.buildChip) {
    const phase = state.buildChip.querySelector("[data-phase]");
    if (phase) phase.textContent = "stopping…";
  }
  // failBuild with the "stopped" reason renders the stopped screen and settles
  // the chip once; the settle-once guard makes a later real exit harmless.
  failBuild("stopped", state.lastRequest || "");
}

$("stopBtn").addEventListener("click", stopBuild);
$("openBtn").addEventListener("click", openApp);
$("shareBtn").addEventListener("click", openPublishSheet);
$("pubGo").addEventListener("click", publishFromSheet);
$("pubShotPick").addEventListener("click", () => pickPublishImage("shot"));
$("pubIconPick").addEventListener("click", () => pickPublishImage("icon"));
$("infoBtn").addEventListener("click", showInfo);
$("revealBtn").addEventListener("click", async () => {
  const app = currentApp();
  if (!app) return;
  try {
    await invoke("reveal", { path: app.path });
  } catch (err) {
    showActionError(err);
  }
});
$("reportBtn")?.addEventListener("click", openReportSheet);
$("repSend")?.addEventListener("click", sendReport);
$("repReveal")?.addEventListener("click", () => {
  if (state.report) invoke("reveal", { path: state.report.path });
});
$("retryBtn").addEventListener("click", () => {
  const again = state.lastFailed;
  show("idle");
  if (again) make(again);
});
["agentChip", "agentChip2", "builtByChip"].forEach((id) => {
  const chip = $(id);
  if (chip) chip.addEventListener("click", openAiSheet);
});
$("buildingNowOpen")?.addEventListener("click", () => {
  if (state.buildingSession) openSession(state.buildingSession);
});
$("buildingNowStop")?.addEventListener("click", stopBuild);
$("settingsBtn").addEventListener("click", openSettings);
$("accountBtn").addEventListener("click", openAccount);
$("changeDirBtn").addEventListener("click", async () => {
  const dir = await invoke("pick_folder");
  if (dir) {
    state.outDir = dir;
    $("outDirValue").textContent = dir;
    await invoke("settings_set", { settings: { out_dir: dir, agent: state.agent } });
  }
});
$("logoutBtn").addEventListener("click", async () => {
  await invoke("account_logout");
  state.account = null;
  // The button lives on the ACCOUNT sheet; hiding only the settings sheet
  // left the popup floating over the sign-in gate after sign-out.
  $("accountSheet").classList.add("hidden");
  $("settingsSheet").classList.add("hidden");
  $("gateStart").classList.remove("hidden");
  $("gateCode").classList.add("hidden");
  showView("gate");
});
document.querySelectorAll(".sheet-close").forEach((b) =>
  b.addEventListener("click", () => $(b.dataset.close).classList.add("hidden")),
);
document.querySelectorAll(".sheet-wrap").forEach((w) =>
  w.addEventListener("click", (e) => { if (e.target === w) w.classList.add("hidden"); }),
);

/* Dragging the window by its title bar.
 *
 * `-webkit-app-region: drag` did not move the window through several
 * attempts. Tauri's `startDragging` hands the drag to the window manager,
 * which is what a native title bar does internally.
 *
 * Delegated from `document` rather than bound per `.titlebar`: the bars live
 * inside views that are hidden at load, and a per-element binding also has to
 * be redone whenever markup is re-rendered. One capturing listener cannot miss
 * one. `capture: true` so it runs before anything inside the bar can stop the
 * event.
 *
 * Failures are logged rather than swallowed. A silent no-op is exactly how
 * this bug survived three rounds of "fixed". */
document.addEventListener("mousedown", (e) => {
  if (e.button !== 0) return;
  const bar = e.target.closest && e.target.closest(".titlebar");
  if (!bar) return;
  if (e.target.closest("button, input, a, .agent-chip")) return;
  const win = tauri && tauri.window &&
    (tauri.window.getCurrentWindow ? tauri.window.getCurrentWindow() : tauri.window.appWindow);
  if (!win || !win.startDragging) {
    console.warn("[drag] no window handle; the title bar cannot move the window");
    return;
  }
  e.preventDefault();
  Promise.resolve(win.startDragging()).catch((err) => {
    console.warn("[drag] startDragging refused:", err);
  });
}, true);

document.addEventListener("dblclick", (e) => {
  const bar = e.target.closest && e.target.closest(".titlebar");
  if (!bar || e.target.closest("button, input, a, .agent-chip")) return;
  const win = tauri && tauri.window &&
    (tauri.window.getCurrentWindow ? tauri.window.getCurrentWindow() : tauri.window.appWindow);
  if (win && win.toggleMaximize) Promise.resolve(win.toggleMaximize()).catch(() => {});
}, true);

if (tauri) {
  // A rejected listen was invisible once: the capabilities grant was
  // missing, invokes worked, and the build screen froze on stage one with
  // an empty log. Surfacing the rejection into the log is the tripwire.
  tauri.event.listen("engine-line", (e) => onEngineLine(e.payload))
    .catch((err) => onEngineLine(`(!) event channel failed: ${err}`));
  tauri.event.listen("login-step", (e) => onLoginStep(e.payload))
    .catch(() => {});
  // Install progress, so a two-minute npm run is not a frozen button. The
  // last line is enough: nobody wants npm's full output, they want to see
  // that something is happening.
  tauri.event.listen("agent-install", (e) => {
    const row = document.querySelector(".ai-row .ai-detail.installing");
    if (row) row.textContent = String(e.payload).slice(0, 90);
  }).catch(() => {});
}

/* ---- mock backend: design-review mode only ---------------------------- */

async function mockInvoke(cmd, args) {
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  switch (cmd) {
    case "account_status":
      return { signed_in: true, login: "you", name: "You" };
    case "account_login":
      onLoginStep({ step: "code", code: "ABCD-1234", url: "github.com/login/device" });
      await sleep(1800);
      onLoginStep({ step: "done", login: "you", name: "You" });
      return;
    case "settings_get":
      return { out_dir: "~/Documents/Krate Apps", agent: "claude" };
    case "settings_set":
      return;
    case "sessions_list":
      return [
        { id: "s-1", title: "a habit tracker", created: 0, updated: Math.floor(Date.now() / 1000) - 7200,
          messages: [{ who: "YOU", body: "a habit tracker", files: [] }, { who: "KRATE", body: "done · habit-tracker.krate · 24 KB" }],
          result: { path: "/tmp/h.krate", name: "habit-tracker.krate", size: "24 KB", asks: ["ui.window:create", "store.kv"], shot: mockShot() } },
        { id: "s-2", title: "a tip splitter for dinners", created: 0, updated: Math.floor(Date.now() / 1000) - 200000,
          messages: [{ who: "YOU", body: "a tip splitter for dinners", files: [] }], result: null },
      ];
    case "session_save":
    case "session_delete":
      return;
    case "agents":
      return [
        { name: "claude", label: "Claude", state: "working", detail: "", remedy: null },
        { name: "codex", label: "Codex", state: "working", detail: "", remedy: null },
        { name: "gemini", label: "Gemini", state: "missing", detail: "not installed", remedy: "npm install -g @google/gemini-cli" },
      ];
    case "pick_files":
      return ["/Users/you/Desktop/sketch.png"];
    case "pick_folder":
      return "/Users/you/Documents/Krate Apps";
    case "create_app":
    case "revise_app": {
      const lines = [
        "==> asking your AI to write the app",
        "  7. writing src/lib.rs (agent)",
        "==> building the component",
        "==> packing the app",
        "==> verifying the permission wall",
      ];
      // Slow enough to walk away from and come back to -- the browsable
      // mock exists to judge exactly that journey, and a three-second
      // "build" cannot show it.
      for (const l of lines) { onEngineLine(l); await sleep(2500); }
      return { path: "/tmp/h.krate", name: "habit-tracker.krate", size: "24 KB",
               asks: ["ui.window:create", "store.kv", "io.args"], shot: mockShot() };
    }
    case "publish":
      await sleep(700);
      return "https://hub.krate.tech/a/f2deb8a76496";
    default:
      return;
  }
}

function mockShot() {
  const c = document.createElement("canvas");
  c.width = 1040; c.height = 640;
  const g = c.getContext("2d");
  g.fillStyle = "#101218"; g.fillRect(0, 0, 1040, 640);
  g.fillStyle = "#ffffff"; g.font = "500 34px Geist, sans-serif";
  g.fillText("Habits", 48, 76);
  const rows = ["Read 20 minutes", "Walk", "No sugar", "Sleep by 11"];
  rows.forEach((t, i) => {
    g.fillStyle = "#16171b";
    g.beginPath(); g.roundRect(48, 120 + i * 92, 944, 72, 14); g.fill();
    g.strokeStyle = i < 2 ? "#6cf4d7" : "#2e323a";
    g.lineWidth = 2;
    g.beginPath(); g.roundRect(76, 142 + i * 92, 28, 28, 8); g.stroke();
    if (i < 2) { g.fillStyle = "#6cf4d7"; g.font = "18px sans-serif"; g.fillText("✓", 82, 163 + i * 92); }
    g.fillStyle = "#ffffff"; g.font = "400 19px Geist, sans-serif";
    g.fillText(t, 128, 166 + i * 92);
  });
  return c.toDataURL();
}

startRotator();
setupDivider();

boot().then(async () => {
  // Automation hook: KRATE_STUDIO_AUTORUN makes the studio drive one real
  // request the moment it opens -- an end-to-end test without faking a
  // keyboard. Unset for people.
  try {
    const auto = await invoke("autorun");
    if (auto && state.account) {
      $("homePrompt").value = auto;
      startFromHome();
    }
  } catch (e) {}
});
