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

/* The stages, shaped to where the time ACTUALLY goes.
 *
 * Measured across every real build (traces in ~/.krate/studio/builds):
 *
 *   reading Krate's API before the first line of code   272-696s  (5-11 min)
 *   writing + checking, the real loop                   the middle
 *   build + pack + permission wall, all three together    0-55s
 *
 * The old five stages lied in both directions. "Writing the code" lit up on the
 * engine's very first line and then sat there for ten minutes while the AI was
 * really READING -- so the longest, most opaque part of a build was labelled as
 * something else, and it read as frozen. Meanwhile three of the five stages
 * covered the last forty seconds and flickered past together.
 *
 * So: name the reading, because it is most of the wait and a person deserves to
 * know that is what is happening; keep writing and testing separate, because
 * they alternate and seeing which one is live is the useful signal; and merge
 * build/pack/wall into one "finishing" step, because to a person they are one
 * moment at the end.
 */
/* How many log lines a build keeps in state. The pane shows the tail, and a
 * long build prints thousands, so holding all of them would grow unbounded
 * for no benefit. */
const BUILD_LOG_LINES = 400;

const STAGES = [
  { key: "read",  label: "Reading Krate's API" },
  { key: "write", label: "Writing the code" },
  { key: "test",  label: "Testing it" },
  { key: "done",  label: "Finishing up" },
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
  /* The live build's progress, held in STATE and not only in the DOM.
   *
   * Leaving the session view (to Cloud, say) and coming back used to show an
   * empty progress pane: the stage list, the log and the current line were
   * written straight into #stateBuilding and nothing kept a copy, so
   * re-entering un-hid a card whose content had been wiped. The build was
   * still running -- the person could see it was active -- and the right-hand
   * side said "you'll see the app preview here" (K-152).
   *
   * Keyed by session id so more than one build can be in flight at once and
   * each keeps its own progress. */
  builds: new Map(),    // sessionId -> { lines, stageIndex, nowLine, title, expect, startedAt }
};

/* The progress record for a session, created on demand. */
function buildRecord(id) {
  if (!id) return null;
  let rec = state.builds.get(id);
  if (!rec) {
    rec = { lines: [], stageIndex: -1, nowLine: "", title: "", expect: "", startedAt: 0, shot: "" };
    state.builds.set(id, rec);
  }
  return rec;
}

/* The record for whichever build is running right now. */
function liveRecord() {
  return state.buildingSession ? buildRecord(state.buildingSession.id) : null;
}

/* ---- views ------------------------------------------------------------ */

function showView(name) {
  for (const id of [
    "viewGate", "viewHome", "viewSession", "viewCloud", "viewApp",
    "viewApps", "viewSettings", "viewProfile", "viewOnboard",
  ]) {
    $(id).classList.add("hidden");
  }
  const view = $({
    gate: "viewGate", home: "viewHome", session: "viewSession",
    cloud: "viewCloud", appDetail: "viewApp", apps: "viewApps",
    settings: "viewSettings", profile: "viewProfile", onboard: "viewOnboard",
  }[name]);
  view.classList.remove("hidden");
  revealIn(view);
  syncDock(name);
}

/* ---- the dock ----------------------------------------------------------
 * One element for the whole app, so switching pages never moves it. It
 * shows on the pages a person navigates between and hides during a build,
 * where the only way out is the session's own back button.
 *
 * The sliding pill is measured, not guessed: a label's width changes with
 * its text, so the highlight reads offsetLeft/offsetWidth of whatever is
 * selected rather than assuming equal buttons. */
const DOCK_PAGES = { home: "home", apps: "apps", cloud: "cloud", settings: "settings" };

function syncDock(view) {
  const wrap = $("dockwrap");
  if (!wrap) return;
  const shown = view === "home" || view === "apps" || view === "cloud"
    || view === "settings" || view === "profile";
  // Onboarding is a single path with its own buttons; a nav bar there
  // invites people to wander out of the one flow that explains the app.
  wrap.classList.toggle("hidden", !shown);
  if (!shown) return;

  const buttons = [...document.querySelectorAll("#dock button[data-page]")];
  const profile = $("dockProfile");
  buttons.forEach((b) => b.classList.remove("on"));
  profile.classList.toggle("on", view === "profile");

  const match = buttons.find((b) => DOCK_PAGES[b.dataset.page] === view);
  if (match) {
    match.classList.add("on");
    moveGlide(match);
  } else {
    // profile: the pill has nothing to sit under, so it collapses away
    $("dockGlide").style.width = "0px";
  }
  document.body.classList.remove("scrolled");
}

function moveGlide(button) {
  const glide = $("dockGlide");
  if (!glide || !button) return;
  glide.style.width = `${button.offsetWidth}px`;
  glide.style.transform = `translateX(${button.offsetLeft - 5}px)`;
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
      // Signed in, but possibly never onboarded -- somebody who signed in
      // on an older build has an account and has still never been told
      // what a .krate is.
      if (needsOnboarding()) {
        showView("onboard");
        obGo(1);
        return;
      }
      enterHome();
      return;
    }
    // Not signed in. That is no longer a wall: nothing but publishing
    // needs an account, so a first run goes to onboarding and everyone
    // else goes straight to the prompt box. The sign-in door is inside
    // onboarding (skippable) and on the profile page.
    if (needsOnboarding()) {
      showView("onboard");
      obGo(1);
      return;
    }
    enterHome();
    return;
  } catch (err) {
    // A missing engine shows on the gate too -- there is nowhere better,
    // and a person facing a broken install needs the real reason rather
    // than a sign-in button that can never work.
    $("gateError").textContent = plainWords(err);
    $("gateError").classList.remove("hidden");
    $("loginBtn").disabled = true;
  }
  showView("gate");
  // One action on the first screen. The code path is the fallback for a
  // browser hand-off that is not going to arrive; it appears when the wait
  // says so, or the moment the browser path errors -- not as a second
  // button competing with the first before anything has gone wrong.
  clearTimeout(state.gateFallback);
  state.gateFallback = setTimeout(() => $("loginBtn").classList.remove("hidden"), 15000);
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
  paintGreeting();
  showView("home");
  renderAccount();
  // Settings FIRST, then agents.
  //
  // These two lines were once the other way round, so a request submitted
  // before the async agent probe returned used the default "claude", and when
  // the probe did return, the settings load overwrote its choice. The chip
  // painted "Codex" while state.agent said "claude", and the build failed
  // naming an agent the user never picked. Settings load first, always.
  const settings = await invoke("settings_get");
  state.outDir = settings.out_dir;
  state.agent = settings.agent || "claude";
  // The person's apps come from local disk in milliseconds; the agent probe
  // runs real tools and can take seconds (twenty, on a machine with one
  // broken provider). Awaiting the probe here meant every launch stared at a
  // home screen with no apps on it for that long. Paint the apps NOW; the
  // chip says "checking…" until the probe lands and updates it. A request
  // submitted before then is safe: runPlan awaits ensureUsableAgent, which
  // finishes the same probe before any agent is asked to work.
  renderSessions(await invoke("sessions_list"));
  renderBuilding();
  refreshAgents();
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
  if (live) {
    $("buildingNowTitle").textContent = state.buildingSession.title;
    // Clear any reveal from-state left on this bar.
    //
    // revealIn() snapshots `.reveal` elements when a view appears and sets
    // opacity:0 on each, then animates them back. But this bar is still
    // `hidden` at that moment -- renderBuilding un-hides it a couple of
    // awaits later, after the animation has already run and finished. So it
    // could come back invisible, or mid-transform: a live build that the
    // person knows is running, with nothing on the home page to show for it
    // (K-152). It is un-hidden after the fact, so it restores itself.
    bar.style.opacity = "1";
    bar.style.transform = "none";
  }
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
  // The grid lives on its own page now. Startup still calls this before
  // that page has ever been shown, which is fine -- but a guard means a
  // future layout change cannot turn a missing element into a thrown
  // error on the path that boots the app.
  if (!grid) return;
  grid.innerHTML = "";
  // Cards are added below with .reveal; stagger them once the grid is built.
  setTimeout(() => revealIn(grid), 0);

  // The library's rules: only things that can be Opened or Sent. A packed
  // file gets a tile; a failed, cancelled, or still-making attempt does
  // not -- three retries of one sentence used to render as three blank
  // graves, and a first-time user read that as a messy product, not as
  // having typed twice. One tile per FILE (newest session per path wins),
  // and the newest unfinished attempt becomes one banner, not a card.
  const files = [];
  const seenPaths = new Set();
  const unfinished = [];
  for (const s of [...sessions].sort((a, b) => (b.updated || 0) - (a.updated || 0))) {
    if (s.result && s.result.path) {
      if (!seenPaths.has(s.result.path)) {
        seenPaths.add(s.result.path);
        files.push(s);
      }
    } else if (s.messages && s.messages.some((m) => m.who === "YOU")) {
      unfinished.push(s);
    }
  }

  const buildingId = state.buildingSession && state.buildingSession.id;
  const resume = unfinished.find((s) => s.id !== buildingId);
  if (resume) {
    const banner = document.createElement("button");
    banner.className = "apps-finish reveal";
    banner.innerHTML = `<span>Finish <b></b></span><span class="go">&rarr;</span>`;
    banner.querySelector("b").textContent = resume.title || "your last app";
    banner.addEventListener("click", () => openSession(resume));
    grid.appendChild(banner);
  }

  if (!files.length && !resume) {
    grid.innerHTML = `<p class="apps-empty">Nothing yet. Your first app is one sentence away.</p>`;
    return;
  }
  for (const s of files) {
    const card = document.createElement("button");
    card.className = "app-card";
    const size = s.result && s.result.size ? ` · ${s.result.size}` : "";
    const hasShot = s.result && s.result.shot;
    card.innerHTML = `<div class="thumb-well${hasShot ? "" : " blank"}"></div>
      <div class="card-body"><p class="name"></p><p class="meta"></p></div>`;
    const well = card.querySelector(".thumb-well");
    if (hasShot) {
      const img = document.createElement("img");
      img.alt = "";
      // Shots live beside the session as PNG files now; "file" is the
      // marker. Loaded lazily per card so the grid itself paints in
      // milliseconds however many apps exist. Old sessions still carry an
      // inline data URL and use it directly.
      if (s.result.shot === "file") {
        invoke("session_shot", { id: s.id })
          .then((data) => { img.src = data; })
          .catch(() => { well.classList.add("blank"); });
      } else {
        img.src = s.result.shot;
      }
      well.appendChild(img);
    } else {
      well.textContent = "open to pick up where you left off";
    }
    const fileName = ((s.result && s.result.name) || s.title || "")
      .replace(/\.krate$/, "")
      .replace(/[-_]+/g, " ")
      .replace(/\b\w/g, (c) => c.toUpperCase());
    card.querySelector(".name").textContent = fileName || s.title;
    card.querySelector(".meta").textContent =
      `${(s.title || "").slice(0, 60)} · ${timeAgo(s.updated)}${size}`;
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
  if (d < 3600) return `${Math.max(1, Math.floor(d / 60))} min ago`;
  if (d < 86400) return `${Math.max(1, Math.floor(d / 3600))} h ago`;
  const days = Math.round(d / 86400);
  return days <= 1 ? "yesterday" : `${days} days ago`;
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
  const msgs = state.session.messages;
  // The last recorded question in a session that never built gets its
  // Build button back on replay. Without this, reopening showed the
  // questions wordlessly and the only affordance left was typing.
  const lastAsk = !building && !s.result
    ? msgs.map((m, i) => (m.kind === "ask" ? i : -1)).filter((i) => i >= 0).pop()
    : undefined;
  // After a restart the in-memory planning state is gone; rebuild enough of
  // it from the transcript that Build it still means "build what I asked
  // for". The request is the person's first message.
  if (lastAsk !== undefined && !state.planning) {
    const first = msgs.find((m) => m.who === "YOU");
    if (first) {
      state.planning = {
        request: first.body,
        files: [],
        qa: [],
        rounds: 1,
        lastQuestions: [],
      };
    }
  }
  msgs.forEach((m, i) => {
    if (i === lastAsk && state.planning) {
      appendMessage(m.who, m.body, m.files, {
        variant: "ask",
        actions: [{ label: "Build it", primary: true, run: finishPlanningAndBuild }],
      });
    } else {
      appendMessage(m.who, m.body, m.files);
    }
  });
  if (building) {
    show("building");
    // Put the live progress back: stages, log, current line and clock. Without
    // this the pane is whatever was left in the DOM, which after a trip to
    // Cloud is nothing at all (K-152).
    restoreBuild(state.session.id);
  } else if (state.session.result) {
    const r = state.session.result;
    if (r.shot === "file") {
      // Draw the card now, pixels a beat later: the shot is on local disk.
      r.shot = "";
      fillDone(r, { reveal: false });
      invoke("session_shot", { id: state.session.id })
        .then((data) => { r.shot = data; fillDone(r, { reveal: false }); })
        .catch(() => {});
    } else {
      fillDone(r, { reveal: false });
    }
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
  share.addEventListener("click", openSendSheet);
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

/* Put a live build's progress back on screen after leaving and returning.
 *
 * `show("building")` only un-hides the pane -- it does not rebuild anything,
 * because everything the pane shows was written directly into the DOM as it
 * happened. Going to Cloud and back therefore left a running build looking
 * dead: no stages, no log, and the idle placeholder still reading "you'll see
 * the app preview here" (K-152). This replays the record kept in state.
 *
 * Defensive throughout, for the same reason beginBuild is: this runs while a
 * build is in flight, and a throw here must never be able to disturb it. */
function restoreBuild(sessionId) {
  const rec = state.builds.get(sessionId);
  if (!rec) return;
  try {
    if (rec.title) $("buildTitle").textContent = rec.title;
    if (rec.expect) $("buildExpect").textContent = rec.expect;

    // Rebuild the stage list, then light it up to where the build has got to.
    //
    // The peek box needs more than beginBuild's rescue here. advanceStage
    // moves #peekBox inside #stages, so by the time a person navigates away
    // the peek -- and #nowLine inside it -- may already have been destroyed
    // with an earlier wipe of that list. Recreating it is the only way back:
    // rescuing assumes it still exists, and on this path it often does not.
    const peekHome = $("stateBuilding")?.querySelector(".build-card") || null;
    let peek = $("peekBox");
    if (!peek && peekHome) {
      peek = document.createElement("div");
      peek.className = "bpeek";
      peek.id = "peekBox";
      peek.innerHTML = '<span id="nowLine"></span><span class="caret"></span>';
      const stagesEl = $("stages");
    if (!stagesEl) { /* the step list is gone on purpose */ } else
      // Back where the markup puts it; advanceStage moves it under the live
      // row again below.
      if (stagesEl && stagesEl.parentElement === peekHome) {
        stagesEl.insertAdjacentElement("afterend", peek);
      } else {
        peekHome.appendChild(peek);
      }
    } else if (peek && peekHome && peek.parentElement !== peekHome) {
      peekHome.appendChild(peek);
    }
    const _stagesEl = $("stages");
  if (_stagesEl) _stagesEl.innerHTML = STAGES.map(
      (s) => `<li data-key="${s.key}"><span class="tick"></span>${s.label}</li>`,
    ).join("");
    const idx = rec.stageIndex;
    document.querySelectorAll("#stages li").forEach((li, i) => {
      li.className = i < idx ? "done" : i === idx ? "now" : "";
    });

    const log = $("buildLog");
    if (log) {
      log.textContent = rec.lines.join("\n");
      if (rec.lines.length) log.textContent += "\n";
      log.scrollTop = log.scrollHeight;
    }
    const now = $("nowLine");
    if (now && rec.nowLine) now.textContent = rec.nowLine;

    // The latest frame the AI rendered, if one has appeared.
    const shotBox = $("buildShotBox");
    const shotImg = $("buildShot");
    if (shotBox && shotImg) {
      if (rec.shot) {
        shotImg.src = rec.shot;
        shotBox.classList.remove("hidden");
      } else {
        shotBox.classList.add("hidden");
      }
    }

    // The clock keeps counting from when the build really started, not from
    // when the person came back to look at it.
    if (rec.startedAt) {
      state.startedAt = rec.startedAt;
      const secs = Math.floor((Date.now() - rec.startedAt) / 1000);
      const el = $("elapsed");
      if (el) {
        el.textContent = `${Math.floor(secs / 60)}:${String(secs % 60).padStart(2, "0")}`;
      }
    }
  } catch (err) {
    // A broken restore must never take the build with it.
    console.warn("restoreBuild failed:", err);
  }
}

function beginBuild(title, expect) {
  $("buildTitle").textContent = title;
  $("buildExpect").textContent = expect;
  // The composer waits until v1 is a file. Typing mid-build either got
  // ignored or read as a new request; a locked box with honest words is
  // kinder than either.
  const box = $("prompt");
  if (box) {
    box.disabled = true;
    box.placeholder = "Wait - v1 is becoming a file…";
  }
  // Rescue the peek box before wiping the stage list.
  //
  // THE SECOND-BUILD FREEZE (K-136). advanceStage MOVES #peekBox inside
  // #stages, tucked under the current row. So on the next build this
  // innerHTML wipe deleted the peek -- and with it #nowLine -- and the
  // "warming up" line below then threw "Cannot set properties of null",
  // which killed buildNow BEFORE it invoked create_app. The build silently
  // never started: no engine, no workspace, no error, and a UI stuck on
  // "building" forever. The first build always worked (peek still in its
  // original home) and every build after it died, which is exactly the
  // alternating pattern the founder hit for days.
  const peek = $("peekBox");
  const peekHome = $("stateBuilding")?.querySelector(".build-card") || null;
  if (peek && peekHome && peek.parentElement !== peekHome) {
    peekHome.appendChild(peek);
  }
  const _stagesEl = $("stages");
  if (_stagesEl) _stagesEl.innerHTML = STAGES.map(
    (s) => `<li data-key="${s.key}"><span class="tick"></span>${s.label}</li>`,
  ).join("");
  $("buildLog").textContent = "";
  const shotBox = $("buildShotBox");
  if (shotBox) shotBox.classList.add("hidden");
  state.stageIndex = -1;
  state.firstTimeSetupSaid = false;
  // Start this session's progress record fresh. Everything the pane shows is
  // mirrored here so re-entering the session can rebuild it (K-152).
  const rec = state.buildingSession ? buildRecord(state.buildingSession.id) : null;
  if (rec) {
    rec.lines = [];
    rec.stageIndex = -1;
    rec.nowLine = "";
    rec.title = title;
    rec.expect = expect;
    rec.startedAt = Date.now();
    rec.shot = "";
  }
  advanceStage("read");
  state.startedAt = Date.now();
  clearInterval(state.timer);
  state.lastLineAt = Date.now();
  let thinkIdx = 0;
  state.timer = setInterval(() => {
    const s = Math.floor((Date.now() - state.startedAt) / 1000);
    const el = $("elapsed");
    if (el) {
      el.textContent = `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
    }
    if (Date.now() - state.lastLineAt > 18000) {
      // Defensive: a missing peek must never throw inside a timer either --
      // an exception here would kill the clock for the rest of the build.
      const now = $("nowLine");
      if (now) now.textContent = thinkingLine(thinkIdx++);
      state.lastLineAt = Date.now() - 8000; // rotate every ~10s while quiet
    }
  }, 1000);
  // Defensive on purpose: nothing in this function may throw, because a throw
  // here happens BEFORE the build is invoked and loses it silently.
  const nowLine = $("nowLine");
  if (nowLine) nowLine.textContent = "warming up…";
  show("building");
}

/* One rail line when the work genuinely shifts -- not for every stage, or the
 * conversation fills with narration. Reading -> writing is the moment the
 * person has been waiting for, and finishing is the moment it is nearly done. */
const STAGE_SAID = {};

/* Lines that mean "a window is about to appear on your screen". When one of
   these is the live step, the build card says so plainly, because the flash
   and the sound arrive with it. */
const FLASH_WORDS = /opening your app|running your app|looking at how your app/i;

function advanceStage(key) {
  const idx = STAGES.findIndex((s) => s.key === key);
  if (idx <= state.stageIndex) return;
  state.stageIndex = idx;
  // Remembered per session, so re-entering restores the lit step (K-152).
  const rec = liveRecord();
  if (rec) rec.stageIndex = idx;
  // Two milestones worth saying out loud. Not five -- a chat that narrates
  // every step is noise, and the stage list already shows all of them.
  if (state.buildChip) {
    const bar = state.buildChip.querySelector(".vbar i");
    if (bar) bar.style.transform = `scaleX(${(idx + 0.5) / STAGES.length})`;
  }
  setProgress((idx + 0.5) / STAGES.length);
}

/* Progress lives on the dock icon (macOS) and the taskbar button (Windows),
 * because the person tabs away from a ten-minute build -- the icon is what
 * they can still see. It tracks real stages, never a timer pretending to
 * know how long an AI will think, and it stops at 92% until the app
 * actually exists: a bar sitting at 100% while nothing has finished is a
 * lie the person can see through. */
function setProgress(fraction) {
  const pct = Math.max(4, Math.min(92, Math.round(fraction * 100)));
  invoke("build_progress", { pct }).catch(() => {});
}

/* Clear the icon's bar -- the build ended, whichever way. */
function clearProgress(done) {
  if (done) {
    invoke("build_progress", { pct: 100 }).catch(() => {});
    setTimeout(() => invoke("build_progress", { pct: null }).catch(() => {}), 1500);
  } else {
    invoke("build_progress", { pct: null }).catch(() => {});
  }
}

/* The agent's own latest test frame, painted as it appears: the person
 * watches their app take shape instead of a static list (see
 * watch_build_shots in the shell). Kept in the session's build record so
 * leaving and coming back restores it -- the K-152 lesson. */
function onBuildShot(dataUrl) {
  const rec = liveRecord();
  if (!rec) return;
  rec.shot = dataUrl;
  const watching =
    state.session && state.buildingSession && state.session.id === state.buildingSession.id;
  if (!watching) return;
  const box = $("buildShotBox");
  const img = $("buildShot");
  if (box && img) {
    img.src = dataUrl;
    box.classList.remove("hidden");
  }
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
  // Keep the line in state as well as on screen, so re-entering the session
  // can rebuild the whole log rather than showing an empty pane (K-152).
  const rec = liveRecord();
  if (rec) {
    rec.lines.push(line);
    // Bounded: a long build can print thousands of lines, and the pane only
    // ever shows the tail. Holding every line would grow without limit.
    if (rec.lines.length > BUILD_LOG_LINES) {
      rec.lines.splice(0, rec.lines.length - BUILD_LOG_LINES);
    }
  }
  const log = $("buildLog");
  if (log) {
    log.textContent += line + "\n";
    log.scrollTop = log.scrollHeight;
  }
  const clean = line.replace(/^=+>\s*/, "").trim();
  if (clean) {
    // Only lines written for a person reach the visible peek. Raw engine
    // steps ("reading bindings.rs", crate names, file paths) stay in the
    // full log where they belong; on the face they re-smell of compiler.
    const HUMAN = /^(reading |writing |checking |opening your app|looking at how|packing |building it|running your app|testing )/i;
    const human = HUMAN.test(clean) && !/\.(rs|toml|wasm|lock)\b/i.test(clean);
    if (human && rec) rec.nowLine = clean;
    const now = $("nowLine");
    if (human && now) now.textContent = clean;
    state.lastLineAt = Date.now();
    // A window is about to appear (or just did). Mark the card so the flash
    // and the sound have a visible explanation at the moment they happen.
    const card = document.querySelector("#stateBuilding .build-card");
    if (card) card.classList.toggle("flashing", FLASH_WORDS.test(clean));
  }
  // The one wait that is not the AI: the very first build on a machine sets
  // up the build tools. The engine says so in its own words ("needs a
  // compiler ... sets up once"); name that moment plainly instead of letting
  // five silent minutes sit under "Reading Krate's API". Once per build.
  if (!state.firstTimeSetupSaid && /needs a compiler|Still to install:/i.test(clean)) {
    state.firstTimeSetupSaid = true;
    sayTo(state.buildingSession || state.session, "KRATE",
      "First-time setup: getting the build tools ready. About five " +
      "minutes, and only this once -- every app you make after this " +
      "starts fast.");
    const expect = $("buildExpect");
    if (expect) expect.textContent = "first-time setup -- about five minutes, only once";
    if (state.buildChip) {
      const phase = state.buildChip.querySelector("[data-phase]");
      if (phase) phase.textContent = "first-time setup (once)";
    }
  }
  // Drive the stages from what the AI is really doing, in the order it really
  // does it. The engine's progress vocabulary is the source: "reading ...",
  // "writing the app's code", "checking it builds ...". Stages only move
  // forward (advanceStage ignores going back), so the write/check alternation
  // settles on the furthest point reached rather than flickering.
  if (/^\s*\d*\.?\s*reading /i.test(clean) || /authoring|starter/i.test(line)) {
    advanceStage("read");
  }
  if (/writing (the app's code|a file)|writing .*\.rs|setting up the build|declaring what the app needs/i.test(clean)) {
    advanceStage("write");
  }
  if (/checking it builds|running your app to test|opening your app to see|looking at how your app/i.test(clean)) {
    advanceStage("test");
  }
  if (/==> building|Compiling|Generating bindings/i.test(line)) advanceStage("test");
  // Packing and the permission wall are the last seconds of a ten-minute
  // build; to a person they are one moment, so they share one step.
  if (/==> packing|==> verifying/.test(line)) advanceStage("done");
}

/* When the engine goes quiet -- an AI thinking is real silence -- the
 * heartbeat keeps beating with honest words, so quiet never looks dead.
 *
 * Keyed by the step that is actually lit. One flat list used to rotate
 * "the writing starts once it has read enough" underneath a lit "Writing the
 * code", which reads as the app contradicting itself and makes a person doubt
 * the whole display. Whatever the heartbeat says has to be true of the step
 * the person is looking at. */
const THINKING = {
  read: [
    "reading Krate's API reference - this part is quiet",
    "still reading - this is the longest part of a build",
    "working through the examples",
    "the writing starts once it has read enough",
  ],
  write: [
    "writing your app's code - this part is quiet",
    "still writing - a whole app is a lot of code",
    "working through the details",
    "nothing is stuck - long silences are normal here",
  ],
  test: [
    "building and testing your app",
    "still testing - it fixes what it finds",
    "compiling takes a minute on its own",
  ],
  done: [
    "packing everything into one file",
    "nearly there",
  ],
};

/* The quiet line for the step that is lit right now. */
function thinkingLine(index) {
  const key = (STAGES[state.stageIndex] || STAGES[0]).key;
  const lines = THINKING[key] || THINKING.read;
  return lines[index % lines.length];
}

function unlockComposer(placeholder) {
  const box = $("prompt");
  if (box) {
    box.disabled = false;
    box.placeholder = placeholder;
  }
}

function fillDone(result, opts) {
  unlockComposer("Want it different? Say what to change…");
  try { localStorage.setItem("krateMadeOnce", "1"); } catch (e) {}
  $("doneName").textContent = result.name;
  $("doneSize").textContent = result.size;
  $("asks").innerHTML = (result.asks || []).map((a) => `<li>${friendlyAsk(a)}</li>`).join("");
  // The preview is the share object: the still with the card's own caption
  // strip -- filename, size, and ONE human trust line. If someone
  // screenshots this screen, they are screenshotting the card.
  $("capName").textContent = result.name || "";
  $("capSize").textContent = result.size || "";
  $("capTrust").textContent = trustLine(result.asks || []);
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
  clearProgress(true);
  // The result belongs to the session that was building, which is not
  // always the one on screen -- a person can browse other sessions while
  // the AI works. Attaching to state.session put finished apps on the
  // wrong session's card.
  const built = state.buildingSession || state.session;
  built.result = result;
  const mins = Math.round((Date.now() - state.startedAt) / 60000);
  const version = state.buildVersion || (built.builds || 0) + 1;
  built.builds = version;
  settleChipOk(state.buildChip, version, result.size, "", built);
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
  clearProgress(false);
  settleChipBad(state.buildChip, state.buildVersion || 1, () => make(request));
  state.buildChip = null;
  clearInterval(state.timer);
  state.lastFailed = request;
  const built = state.buildingSession || state.session;
  if (built) built.failedRequest = request;
  sayTo(built, "KRATE", why === "stopped" ? "stopped" : "that build didn't come together");
  persistSession(built);
  if (!(state.session && built && state.session.id === built.id)) return;
  /* The one hard rule of this card: plain words. A person here must never
   * meet a compiler error, an exit code, or a crate name. */
  if (why === "stopped") {
    $("failTitle").textContent = "Stopped.";
    $("failWhy").textContent = "Nothing was lost -- your words are kept, ready to send again.";
    $("retryBtn").textContent = "Resume build";
    unlockComposer("Changed your mind? Say it - or hit Resume build");
  } else {
    $("retryBtn").textContent = "Try again";
    unlockComposer("Say it another way, or hit Try again");
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

/* The card's one-line trust sentence: the abilities in plain words, ending
 * with the guarantee. Network absence is stated, because that is the line
 * that makes a stranger dare to open it. */
function trustLine(asks) {
  const PLUMBING = /^(io\.|time\.|locale\.|gfx\.gpu)/;
  const words = [];
  for (const cap of asks) {
    if (PLUMBING.test(String(cap))) continue;
    const w = friendlyAsk(cap);
    if (w && !words.includes(w)) words.push(w);
    if (words.length === 2) break;
  }
  const net = asks.some((c) => String(c).startsWith("net."));
  let line = words.length ? "can " + words.join(" · ") : "asks for nothing beyond a window";
  if (!net) line += " · cannot use the network";
  return line;
}

function friendlyAsk(cap) {
  const map = {
    "ui.window:create": "open a window",
    "io.stdout": "print text",
    "io.args": "read its start-up options",
    "store.kv": "save your data on this computer",
    "store.shared": "share its data with people who have its invite code",
    "store.sql": "keep records on this computer",
    "time.clock": "read the clock",
    "net.http": "reach the internet",
    "fs.read": "read files you choose",
    "fs.write": "save files you choose",
  };
  return map[cap] || map[cap.split(":")[0]] || cap;
}

/* ---- driving the engine ----------------------------------------------- */

async function make(request, opts) {
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
  // A resume or an amend already spoke in the person's own words; echoing
  // the stitched request would print machinery at them.
  if (!(opts && opts.silent)) say("YOU", request, files);
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
    // The planning session's id, when the engine carried one: the build
    // resumes that session, so the request and the agreed plan are already
    // in the AI's context instead of being re-sent to a cold start.
    if (answer.agent_session) state.planning.agentSession = answer.agent_session;
    if (answer.ask && answer.ask.length && state.planning.rounds < 1) {
      // ONE round of questions, ever. The first live session got two
      // rounds and called it what it is: frustrating.
      state.planning.rounds += 1;
      state.planning.lastQuestions = answer.ask;
      const questions = answer.ask.map((q, i) => `${i + 1}. ${q}`).join("\n");
      say("KRATE", questions, null, {
        variant: "ask",
        actions: [{ label: "Build it", primary: true, run: finishPlanningAndBuild }],
      });
      // The recorded message remembers it was a question, so reopening the
      // session can put the Build button back (see openSession) -- a
      // transcript that keeps the questions but loses the button leaves
      // typing as the only path, and a stray word becomes an app (K-196's
      // sibling trap, seen live as a calculator session).
      const rec = state.session.messages[state.session.messages.length - 1];
      if (rec) rec.kind = "ask";
      setIdleNote("Answer on the left, or just hit Build it.");
      $("prompt").placeholder = "Answer here… or hit Build it";
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
      {
        const rec = state.session.messages[state.session.messages.length - 1];
        if (rec) rec.kind = "ask";
      }
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
    // A broken tool must not be reported as a decision. When the engine
    // beside this Studio is too old to plan, "I'll skip the questions this
    // time" describes a choice nobody made, and the person reads it as the
    // product quietly changing behaviour -- which is exactly how K-180 cost
    // an afternoon. Say what is wrong and what fixes it, then still build,
    // because refusing to work would be a worse answer than working without
    // the conversation.
    if (String(err).includes("STALE_ENGINE")) {
      say("KRATE", "The Krate engine on this machine is older than this "
        + "Studio, so I cannot talk an app through before building it. "
        + "Updating Krate restores the questions. I'll build directly for now.");
      setIdleNote("Old engine: building without the planning step.");
      return finishPlanningAndBuild();
    }
    // A question is not a build request. When the plan step cannot answer
    // AND the message reads as a question about Krate, answer it instead
    // of spending fifteen minutes building a reply to it.
    const asked = state.planning && state.planning.request;
    if (looksLikeAQuestion(asked)) {
      say("KRATE", "Yes -- tell me what the app should do and I'll build it. "
        + "A window, saved data, drawing, sound, the network: all fair game. "
        + "One sentence is enough to start.");
      $("prompt").placeholder = "Describe the app you want\u2026";
      setIdleNote("Tell me what to make and I'll start.");
      $("send").disabled = false;
      return;
    }
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
  return buildNow(enriched, p.files, false, p.agentSession || "");
}

async function buildNow(request, files, revising, planSession) {
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
  if (revising) say("KRATE", "Reading your app, then making that change.");
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
  // The build must survive a broken screen. Everything above this point is
  // presentation -- chips, stages, timers -- and none of it is a reason to
  // lose the person's build. A throw in beginBuild once killed buildNow before
  // it ever invoked the engine, and the app simply never got made (K-136).
  // Draw what we can; the invoke below happens either way.
  try {
    beginBuild(
      revising ? "Making your change" : "Making your app",
      // Honest numbers: measured across real builds (traces in
      // ~/.krate/studio/builds), a fresh app is 5-15 minutes and the
      // median is ~13. "A few minutes" read as a promise and then as a lie.
      revising
        ? "changes are quicker - the AI reads your app first"
        : localStorage.getItem("krateMadeOnce")
          ? "a minute or two, sometimes more"
          : "first time on this Mac - a few minutes",
    );
  } catch (err) {
    console.warn("beginBuild failed, building anyway:", err);
    show("building");
  }

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
          planSession: planSession || "",
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
    // Drop this build's progress record. It exists to survive navigation
    // WHILE a build runs; once the build has settled the session's own
    // result is what the pane shows, and keeping the record would let a
    // long sitting accumulate one per app made.
    if (state.buildingSession) state.builds.delete(state.buildingSession.id);
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

/* Sentences Krate itself adds around a provider's error.
 *
 * They have to come out before anything is matched, because this function
 * decides what the person is told by looking for words in the text -- and
 * our own advice contains those words. "Sign in again, then try once more"
 * is appended by the engine whenever a provider error mentions
 * authentication; the classifier then read OUR sentence and told a person
 * with a working, signed-in, green-dotted Codex that their AI needed
 * signing in (K-184). "This is a problem with the AI tool, not with Krate
 * or your request. Check that `codex` runs on its own" is the same shape:
 * it contains "connect"-adjacent and tool-name words that steer the guess.
 *
 * This is the second time our own prose has been mistaken for evidence --
 * K-124 was the first, and the (?!or) guard below is its scar. Stripping
 * the whole family is the fix that does not need a new guard each time.
 */
const KRATE_OWN_PROSE = [
  /\n?\s*Sign in again, then try once more\.?/gi,
  /This is a problem with the AI tool, not with Krate or your request\.?/gi,
  /Check that `[^`]+` runs on its own, then try again\.?/gi,
  /The full transcript is at [^\s]+\.?/gi,
  /see \/[^\s]+\.agent-transcript\.txt/gi,
  /[a-z]+ could not write the app:/gi,
];

function providerWords(text) {
  let out = String(text);
  for (const pattern of KRATE_OWN_PROSE) out = out.replace(pattern, " ");
  return out;
}

function plainWords(err) {
  const raw = String(err && err.message ? err.message : err);
  if (raw === "stopped") return "stopped";
  // Classify on what the PROVIDER said, never on what Krate said about it.
  const text = providerWords(raw);
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
  // Usage limits read as auth failures to a keyword match but are not one,
  // and telling someone to sign in when they are already signed in is the
  // most confusing thing this card can say. Check for the limit first.
  if (/usage limit|out of credits|insufficient_quota|too many requests|\b429\b/i.test(text))
    return "Your AI has hit its usage limit. It works again once the limit resets, or pick another AI from the menu at the top.";
  // A real authentication failure, judged on the provider's OWN signal: an
  // HTTP 401/403, or the words a provider uses for an expired session. Our
  // own advice was stripped above, so nothing here can match Krate's prose.
  // \bauth catches authentication/unauthorized; the (?!or) guard keeps
  // "author command failed" -- our own generic failure line -- from telling
  // every user to go sign in (K-124: it did exactly that).
  if (/\b401\b|\b403\b|unauthorized|sign ?in|\bauth(?!or)|logged/i.test(text))
    return "Your AI is not signed in, or its sign-in expired. Click its name at the top for the fix.";
  if (/network|offline|dns|connect/i.test(text)) return "The internet connection dropped mid-build.";
  return "Something in the build went wrong. Trying again usually works; your words are kept.";
}

/* ---- agents ----------------------------------------------------------- */


/* ---- the AI marks ------------------------------------------------------
 * Drawn inline rather than shipped as files: five small SVGs cost less
 * than five network-free image assets and inherit the theme's colours.
 * Each is the tool's own recognisable shape, simplified to one flat mark
 * so the row reads at a glance instead of being decoded. */
const AI_LOGOS = {
  claude:
    '<svg viewBox="0 0 24 24" width="18" height="18" aria-hidden="true">' +
    '<path fill="#D97757" d="M6.1 15.6l3.4-1.9.06-.17-.06-.09h-.17l-.58-.04-1.98-.05-1.71-.07-1.66-.09-.42-.09-.39-.51.04-.26.35-.23.5.04 1.11.08 1.66.11 1.2.07 1.79.19h.28l.04-.11-.1-.07-.07-.07-1.68-1.14-1.82-1.2-.95-.7-.52-.35-.26-.33-.11-.72.47-.52.63.04.16.04.64.49 1.36 1.05 1.78 1.31.26.22.1-.07.01-.05-.12-.2-.98-1.77-1.05-1.8-.47-.75-.12-.45a2.2 2.2 0 01-.08-.53l.54-.73.3-.1.72.1.3.26.45 1.03.73 1.62 1.13 2.2.33.65.18.6.07.19h.12v-.11l.1-1.31.18-1.61.18-2.07.06-.58.29-.71.58-.38.45.22.37.53-.05.34-.22 1.45-.44 2.28-.29 1.53h.17l.19-.19.78-1.03 1.3-1.63.58-.65.67-.71.43-.34h.81l.6.89-.27.92-.84 1.06-.69.9-1 1.34-.62 1.07.06.09.15-.01 2.27-.48 1.23-.22 1.46-.25.66.31.07.31-.26.64-1.56.39-1.83.37-2.73.64-.03.02.04.05 1.23.11.53.03h1.29l2.4.18.63.41.37.51-.06.38-.97.49-1.3-.31-3.05-.72-1.04-.26h-.15v.09l.87.85 1.6 1.44 2 1.86.1.46-.26.36-.27-.04-1.78-1.34-.69-.6-1.55-1.31h-.1v.14l.36.52 1.89 2.84.1.87-.14.29-.49.17-.54-.1-1.11-1.56-1.15-1.76-.93-1.58-.11.07-.55 5.9-.26.3-.59.23-.5-.38-.26-.61.26-1.2.32-1.57.26-1.25.23-1.55.14-.51-.01-.04-.11.02-1.16 1.59-1.76 2.38-1.39 1.49-.34.13-.58-.3.05-.53.32-.48 1.94-2.46.68-.89 1.28-1.5-.01-.14h-.05L6.9 18.3l-1.15.15-.5-.47.06-.76.24-.25 1.96-1.35z"/></svg>',
  codex:
    '<svg viewBox="0 0 24 24" width="18" height="18" aria-hidden="true">' +
    '<path fill="currentColor" d="M22.28 9.82a5.98 5.98 0 00-.52-4.91 6.05 6.05 0 00-6.51-2.9A6 6 0 004.98 4.18a5.98 5.98 0 00-4 2.9 6.05 6.05 0 00.74 7.1 5.98 5.98 0 00.51 4.91 6.05 6.05 0 006.52 2.9A5.98 5.98 0 0019.02 19.8a5.98 5.98 0 004-2.9 6.05 6.05 0 00-.74-7.09zm-9.02 12.6a4.48 4.48 0 01-2.88-1.04l.14-.08 4.78-2.76a.79.79 0 00.4-.68v-6.74l2.02 1.17a.07.07 0 01.04.05v5.58a4.5 4.5 0 01-4.5 4.5zM3.6 18.3a4.47 4.47 0 01-.54-3.01l.14.08 4.78 2.76a.78.78 0 00.78 0l5.84-3.37v2.33a.07.07 0 01-.03.06L9.73 19.95a4.5 4.5 0 01-6.14-1.65zM2.34 7.9a4.48 4.48 0 012.34-1.97v5.68a.78.78 0 00.39.67l5.81 3.36-2.02 1.17a.07.07 0 01-.07 0L4 14.03A4.5 4.5 0 012.34 7.9zm16.6 3.86l-5.84-3.4L15.12 7.2a.07.07 0 01.07 0l4.83 2.79a4.49 4.49 0 01-.68 8.1v-5.68a.78.78 0 00-.39-.66zm2.01-3.03l-.14-.09-4.77-2.78a.78.78 0 00-.79 0L9.42 9.24V6.9a.07.07 0 01.03-.06l4.83-2.79a4.5 4.5 0 016.68 4.66zM8.32 12.86L6.3 11.7a.07.07 0 01-.04-.06V6.07a4.5 4.5 0 017.38-3.45l-.14.08L8.72 5.46a.79.79 0 00-.4.68v6.72zm1.1-2.37L12.02 9l2.6 1.5v3l-2.6 1.5-2.6-1.5v-3z"/></svg>',
  gemini:
    '<svg viewBox="0 0 24 24" width="18" height="18" aria-hidden="true">' +
    '<defs><linearGradient id="gemg" x1="0" y1="0" x2="1" y2="1">' +
    '<stop offset="0" stop-color="#4796E3"/><stop offset="0.5" stop-color="#9177C7"/>' +
    '<stop offset="1" stop-color="#D96570"/></linearGradient></defs>' +
    '<path fill="url(#gemg)" d="M12 1.5c.35 4.02 1.9 6.9 4.3 8.55 1.3.9 2.9 1.5 4.7 1.8v.3c-4.02.35-6.9 1.9-8.55 4.3-.9 1.3-1.5 2.9-1.8 4.7h-.3c-.35-4.02-1.9-6.9-4.3-8.55-1.3-.9-2.9-1.5-4.7-1.8v-.3c4.02-.35 6.9-1.9 8.55-4.3.9-1.3 1.5-2.9 1.8-4.7h.3z"/></svg>',
  copilot:
    '<svg viewBox="0 0 24 24" width="18" height="18" aria-hidden="true">' +
    '<path fill="currentColor" d="M12 0C5.37 0 0 5.37 0 12c0 5.31 3.44 9.8 8.21 11.39.6.11.82-.26.82-.58v-2.23c-3.34.73-4.04-1.42-4.04-1.42-.55-1.39-1.34-1.76-1.34-1.76-1.09-.75.08-.73.08-.73 1.2.08 1.84 1.24 1.84 1.24 1.07 1.83 2.81 1.3 3.5.99.11-.78.42-1.31.76-1.61-2.67-.3-5.47-1.33-5.47-5.93 0-1.31.47-2.38 1.24-3.22-.13-.3-.54-1.52.12-3.18 0 0 1.01-.32 3.3 1.23a11.5 11.5 0 016.01 0c2.29-1.55 3.3-1.23 3.3-1.23.66 1.66.24 2.88.12 3.18.77.84 1.23 1.91 1.23 3.22 0 4.61-2.8 5.62-5.48 5.92.43.37.81 1.1.81 2.22v3.29c0 .32.22.7.83.58A12 12 0 0024 12c0-6.63-5.37-12-12-12z"/></svg>',
  grok:
    '<svg viewBox="0 0 24 24" width="18" height="18" aria-hidden="true">' +
    '<path fill="currentColor" d="M3.3 20.7l9.2-9.2 3.3 3.3-6 6H3.3zm0-5.4L14.6 4h4.8L8.1 15.3H3.3zm14.1 5.4V9.9l3.3-3.3v14.1h-3.3z"/></svg>',
};

function aiLogo(name) {
  return AI_LOGOS[name] || '<svg viewBox="0 0 24 24" width="18" height="18" aria-hidden="true">'
    + '<circle cx="12" cy="12" r="8" fill="none" stroke="currentColor" stroke-width="1.6"/></svg>';
}

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
    // Human words on the row; the raw command and any long engine output
    // live under a Terminal fold. A first-timer sees a state and a button,
    // never an npm line or a stack trace.
    const rawDetail =
      a.state === "working" ? (a.name === state.agent ? "ready · in use" : "ready")
      : a.detail || (a.state === "missing" ? "not installed" : "not ready");
    const tooTechnical = a.state !== "working" && rawDetail.length > 80;
    const detail = tooTechnical
      ? (a.state === "missing" ? "needs a one-time install" : "not ready · details under Terminal")
      : rawDetail;
    const foldText = [tooTechnical ? rawDetail : "", a.remedy || ""].filter(Boolean).join("\n");
    row.innerHTML = `
      <span class="ai-mark">${aiLogo(a.name)}<i class="dot ${dot}"></i></span>
      <div class="grow">
        <p class="ai-name"></p>
        <p class="ai-detail"></p>
        ${foldText ? `<details class="ai-terminal"><summary>Terminal</summary><pre class="ai-remedy"></pre></details>` : ""}
      </div>`;
    row.querySelector(".ai-name").textContent = a.label;
    row.querySelector(".ai-detail").textContent = detail;
    if (foldText) row.querySelector(".ai-remedy").textContent = foldText;
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
    } else if (a.state === "not-ready") {
      // Installed but unusable, which almost always means "not signed in".
      // The engine's own detail says why; the button does something about
      // it instead of asking the person to read and retype a command.
      const fix = document.createElement("button");
      fix.className = "btn";
      fix.textContent = "Sign in";
      fix.addEventListener("click", async () => {
        const note = row.querySelector(".ai-detail");
        try {
          await invoke("sign_in_agent", { name: a.name });
          note.textContent = "Finish in the terminal, then check again.";
          fix.textContent = "Check again";
          fix.onclick = () => { refreshAgents(); openAiSheet(); };
        } catch (err) {
          note.textContent = String(err);
        }
      });
      row.appendChild(fix);
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
          note.textContent = "Installed. One sign-in left.";
          // Signing in is interactive, so it needs a terminal. Offering the
          // button beats printing the command and hoping.
          add.textContent = "Sign in";
          add.disabled = false;
          add.onclick = async () => {
            try {
              await invoke("sign_in_agent", { name: a.name });
              note.textContent = "Finish signing in, then come back.";
              add.textContent = "Check again";
              add.onclick = () => { refreshAgents(); openAiSheet(); };
            } catch (err) {
              note.textContent = String(err);
            }
          };
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
  "store.kv": "Save its own settings",
  "store.sql": "Keep its own database",
  "store.secret": "Keep its own sign-in keys, encrypted",
  "store.shared": "Share its data with anyone holding its invite code, via krate.tech",
  "audio.playback": "Play sound",
  "audio.capture": "Record from the microphone",
  "camera.capture": "Use the camera",
  // The engine's names, as printed by `krate run --dump-caps`. Three
  // entries here read "net.http", "clipboard.read" and "clipboard.write",
  // which no capability is called -- so an app that reached the internet
  // or the clipboard showed a raw id like "net.connect:api.site.com:443"
  // in a list whose whole job is plain English.
  "net.connect": "Reach the internet",
  "ui.clipboard:read": "Read the clipboard",
  "ui.clipboard:write": "Put things on the clipboard",
  "ui.open-url": "Open a link in your browser",
  "ui.notify": "Send you desktop notifications",
  "ui.menu:system": "Add items to the system menu",
  "ui.dropzone": "Accept files you drag onto it",
  "ui.dialog:*": "Use any of the file and message dialogs",
  "gfx.gpu:compute": "Run calculations on the graphics card",
  "fs.read": "Read files you allow",
  "fs.write": "Write files you allow",
  "fs.list": "List folders you allow",
  "fs.remove": "Delete files you allow",
  "fs.mkdir": "Create folders you allow",
};

/* A capability may carry a resource -- "net.connect:api.site.com:443",
   "fs.read:./notes/*". The words are keyed on the bare module.action, so
   the resource is stripped before the lookup and shown after the phrase,
   which is the part a person most wants to see: WHICH host, WHICH folder. */
function capWords(cap) {
  if (CAP_WORDS[cap]) return CAP_WORDS[cap];
  const i = cap.indexOf(":");
  if (i === -1) return cap;
  const bare = cap.slice(0, i);
  const resource = cap.slice(i + 1);
  const words = CAP_WORDS[bare];
  return words ? `${words} (${resource})` : cap;
}

async function showInfo() {
  const app = currentApp();
  if (!app) return;
  const sheet = $("infoSheet");
  $("infoName").textContent = app.name || "Your app";
  $("infoTrust").textContent = "Reading the app…";
  $("infoMeta").textContent = "";
  $("infoRows").innerHTML = "";
  $("infoCaps").innerHTML = "";
  $("infoAsks").innerHTML = "";
  sheet.classList.remove("hidden");

  try {
    const info = await invoke("app_info", { path: app.path });

    // The face: one human line and the quiet meta. If someone reads only
    // two lines of this sheet, they read the ones that make it sendable.
    const asks = info.asks || [];
    $("infoTrust").textContent = trustLine(asks.map((a) => a.cap || a.words));
    $("infoMeta").textContent =
      `${Math.round((info.size || 0) / 1024)} KB · Mac, Windows and Linux`;

    for (const a of asks) {
      const li = document.createElement("li");
      li.textContent = a.words;
      li.title = a.cap;
      $("infoAsks").appendChild(li);
    }
    if (!asks.length) {
      const li = document.createElement("li");
      li.textContent = "Nothing beyond drawing its own window.";
      $("infoAsks").appendChild(li);
    }

    for (const cap of info.capabilities || []) {
      const li = document.createElement("li");
      li.textContent = capWords(cap);
      li.title = cap;
      $("infoCaps").appendChild(li);
    }

    // The path and the full fingerprint are for the maker, behind the fold
    // -- on the face they read as machinery and scare the send away.
    const rows = [
      ["Where", info.path],
      ["Fingerprint", (info.identity || "").slice(0, 16) || "unknown"],
    ];
    for (const [k, v] of rows) {
      const dt = document.createElement("dt");
      dt.textContent = k;
      const dd = document.createElement("dd");
      dd.textContent = v;
      $("infoRows").append(dt, dd);
    }
    const copy = $("infoCopyPath");
    if (copy) {
      copy.onclick = async () => {
        try { await navigator.clipboard.writeText(info.path); copy.textContent = "Copied"; } catch (e) {}
        setTimeout(() => { copy.textContent = "Copy the path"; }, 1400);
      };
    }
  } catch (err) {
    $("infoTrust").textContent = String(err);
  }
}

/* ---- Krate Cloud ------------------------------------------------------- */



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
  // The skeleton replaces the old "Reading Krate Cloud…" line: cards in
  // the shape of the answer, in the place the answer lands, so the wait
  // reads as loading rather than as nothing happening. Painted here --
  // after the grid is cleared -- because clearing it first wiped it.
  $("cloudLoading").classList.add("hidden");
  $("cloudGrid").innerHTML = "";
  $("cloudCount").textContent = "";
  showCloudSkeleton();
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

  // The icon sits on the stage, over the screenshot.
  const head = $("detailHead");
  head.innerHTML = "";
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

  $("detailName").textContent = meta.name || "Untitled app";
  $("detailNote").textContent = "";

  // Who made it, with their real photo when the hub has one. An avatar and
  // a name say "a person made this" in a way a line of grey text does not.
  const by = $("detailBy");
  by.innerHTML = "";
  if (meta.avatar_url) {
    const av = document.createElement("img");
    av.className = "ident-avatar";
    av.src = meta.avatar_url;
    av.alt = "";
    av.onerror = () => av.remove();
    by.appendChild(av);
  }
  by.append(document.createTextNode(meta.author || "Someone on Krate"));
  const cat = document.createElement("span");
  cat.className = "ident-cat";
  cat.textContent = catLabel((app.cats && app.cats[0]) || meta.category || "apps");
  by.appendChild(cat);

  const shot = $("detailShot");
  shot.innerHTML = "";
  if (app.shot) {
    // Two copies of one picture: a blurred backdrop that fills the stage,
    // and the real thing at its own shape on top. App windows are anywhere
    // from portrait to landscape, so nothing is cropped to make it fit.
    const bed = document.createElement("img");
    bed.className = "shot-bed";
    bed.src = app.shot;
    bed.alt = "";
    bed.setAttribute("aria-hidden", "true");

    const img = document.createElement("img");
    img.className = "shot";
    img.src = app.shot;
    img.alt = `${meta.name || "The app"}, as it renders`;
    // The hub has no screenshot for every app; a broken image frame looks
    // like a fault, so it removes itself and the placeholder shows instead.
    img.onerror = () => { shot.innerHTML = ""; shot.appendChild(shotPlaceholder()); };

    shot.append(bed, img);
  } else {
    shot.appendChild(shotPlaceholder());
  }

  // The two facts weighed before clicking Open ride beside the button.
  const size = meta.size ? `${Math.round(meta.size / 1024)} KB` : "unknown";
  const when = meta.published ? timeAgo(meta.published) : "unknown";
  $("detailQuick").innerHTML = "";
  for (const [k, v] of [["Size", size], ["Published", when]]) {
    const box = document.createElement("div");
    box.innerHTML = `<span class="k"></span><span class="v"></span>`;
    box.querySelector(".k").textContent = k;
    box.querySelector(".v").textContent = v;
    $("detailQuick").appendChild(box);
  }

  // Most published apps carry no description, so the card must read as
  // finished without one rather than leaving a gap where prose would go.
  $("detailDesc").textContent = meta.description || "";
  $("detailDesc").classList.toggle("hidden", !meta.description);

  const rows = [
    ["Runs on", "Mac, Windows, Linux"],
    ["Download", size],
    ["Published", when],
  ];
  $("detailRows").innerHTML = "";
  for (const [k, v] of rows) {
    const dt = document.createElement("dt");
    dt.textContent = k;
    const dd = document.createElement("dd");
    dd.textContent = v;
    $("detailRows").append(dt, dd);
  }

  const code = $("detailCode");
  $("detailCodeText").textContent = (app.url || "").replace(/^https:\/\//, "");
  code.classList.remove("copied");
  code.classList.toggle("hidden", !app.url);

  // Permissions come from the engine reading the published bundle, without
  // running it -- the same wall that will apply if it is opened.
  const caps = $("detailCaps");
  const count = $("detailCount");
  caps.innerHTML = '<p class="cap-dim">Checking what it can do…</p>';
  count.textContent = "";
  invoke("app_info", { path: app.url })
    .then((info) => {
      const list = info.capabilities || [];
      count.textContent = list.length
        ? `${list.length} permission${list.length === 1 ? "" : "s"}`
        : "";
      renderCapGroups(caps, list);
    })
    .catch(() => {
      caps.innerHTML = '<p class="cap-dim">Could not read this app right now.</p>';
    });
}

/* The permissions that cross the app's own wall: they read the person's
   room, their files, their clipboard, or move data off the machine.
   Everything else the app does to itself and its own window.
 *
 * The split is about DIRECTION, not about whether the engine asks. Some
 * capabilities are an explicit ask purely so the list discloses them
 * (store.kv, store.sql, random.bytes) while nothing leaves the app --
 * those belong under "stays inside" even though they are not
 * default-granted.
 *
 * Names are the engine's own, as printed by `krate run --dump-caps` and
 * defined in KRATE_CAPABILITY_SPECS (crates/manifest/src/lib.rs). Listed
 * one by one rather than matched by prefix: a capability added later
 * should have to be classified deliberately, and an unrecognised one
 * falls to the cautious side rather than being quietly called harmless. */
const CAPS_REACH_OUT = new Set([
  // The person's room, camera and files.
  "audio.capture",
  "camera.capture",
  "ui.clipboard:read",
  "ui.clipboard:write",
  "ui.dialog:file-open",
  "ui.dialog:file-save",
  "ui.dialog:open-folder",
  "ui.dialog:*",
  "ui.dropzone",
  // Off the machine.
  "net.connect",
  "store.shared",
  // Acts on the desktop outside its own window.
  "ui.open-url",
  "ui.notify",
  "ui.menu:system",
  // The filesystem.
  "fs.read", "fs.write", "fs.list", "fs.remove", "fs.mkdir",
]);

/* Capabilities that stay entirely within the app: drawing its own window,
   its own storage, its own output, reading the clock.

   audio.playback is here deliberately. The engine default-grants it with
   the note "Output is not input" -- playing sound is the app doing its
   obvious job, not reaching for anything of the person's. */
const CAPS_STAY_IN = new Set([
  "ui.window:create", "ui.dialog:confirm", "ui.dialog:message",
  "gfx.gpu:basic", "gfx.gpu:compute",
  "io.stdout", "io.stderr", "io.stdin", "io.args", "io.log",
  "time.clock", "time.monotonic", "time.sleep",
  "locale.info", "locale.format", "random.bytes",
  "store.kv", "store.sql", "store.secret",
  "audio.playback",
]);

function reachesOut(cap) {
  if (CAPS_REACH_OUT.has(cap)) return true;
  if (CAPS_STAY_IN.has(cap)) return false;

  // Several capabilities arrive carrying a resource -- "fs.read:./notes/*",
  // "net.connect:api.example.com:443" -- so an exact match alone would call
  // every one of them unknown. Try the bare module.action too, and only
  // then give up.
  const bare = cap.slice(0, cap.indexOf(":") === -1 ? cap.length : cap.indexOf(":"));
  if (CAPS_REACH_OUT.has(bare)) return true;
  if (CAPS_STAY_IN.has(bare)) return false;

  // Unknown to this build of Studio -- a capability newer than this
  // release. Show it in the group that gets read rather than calling
  // something we cannot name harmless.
  return true;
}

/* Fourteen permissions as a stacked bullet list was fourteen lines of
   equal weight, and the one worth noticing hid among the dull ones. As
   pills in two groups it fits in a glance and the split carries meaning. */
function renderCapGroups(host, list) {
  host.innerHTML = "";
  if (!list.length) {
    const p = document.createElement("p");
    p.className = "cap-dim";
    p.textContent = "Nothing beyond drawing its own window.";
    host.appendChild(p);
    return;
  }

  const out = list.filter(reachesOut);
  const inside = list.filter((c) => !reachesOut(c));
  // With only one group the coloured pip and its heading say nothing, so
  // the pills stand alone.
  const solo = !out.length || !inside.length;

  for (const [cls, title, items] of [
    ["out", "Reaches outside the app", out],
    ["in", "Stays inside the app", inside],
  ]) {
    if (!items.length) continue;
    const group = document.createElement("div");
    group.className = `pgroup ${cls}`;
    if (!solo) {
      const head = document.createElement("div");
      head.className = "pgroup-t";
      head.innerHTML = `<span class="pip"></span>`;
      head.append(document.createTextNode(title));
      group.appendChild(head);
    }
    const ul = document.createElement("ul");
    ul.className = "plist";
    for (const cap of items) {
      const li = document.createElement("li");
      li.textContent = capWords(cap);
      li.title = cap;
      ul.appendChild(li);
    }
    group.appendChild(ul);
    host.appendChild(group);
  }
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

function openSendSheet() {
  const app = currentApp();
  if (!app) return;
  $("sendCardDone").classList.add("hidden");
  $("sendNote").textContent = "";
  $("sendCardBtn").disabled = false;
  $("sendSheet").classList.remove("hidden");
}

async function sendCard() {
  const app = currentApp();
  if (!app) return;
  $("sendCardBtn").disabled = true;
  $("sendNote").textContent = "Photographing your app\u2026 a few seconds.";
  try {
    const cardPath = await invoke("make_card", { path: app.path });
    const data = await invoke("read_image", { path: cardPath });
    $("sendCardImg").src = data;
    $("sendCardDone").classList.remove("hidden");
    const name = cardPath.split(/[\\/]/).pop();
    $("sendCardNote").textContent = name +
      " is in the folder that just opened. Drag it into mail, AirDrop, or a " +
      "chat's paperclip -- send it as a file, not as a photo.";
    $("sendNote").textContent = "";
    try { await invoke("reveal", { path: cardPath }); } catch (e) {}
  } catch (err) {
    $("sendNote").textContent = plainWords(err);
    $("sendCardBtn").disabled = false;
  }
}

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
      unlisted: !$("pubListed").checked,
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

  // Double-click restores the default split: a person who drags the rail
  // to an unusable width needs a way back that is not "guess 320px".
  divider.addEventListener("dblclick", () => {
    rail.style.width = rail.style.minWidth = "320px";
    localStorage.setItem("krate.railWidth", "320");
  });

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

/* Does this read as a request to build, or a question about Krate?
 *
 * The engine's plan step can only answer with questions or a plan -- it
 * has no way to say "that was not a request", so anything conversational
 * fell through to a fifteen-minute build. "can we make any app?" produced
 * an app. This catches the shapes people actually type, and it errs
 * toward building: wrongly calling a request a question is far worse than
 * building something somebody half-meant. */
function looksLikeAQuestion(text) {
  const t = String(text || "").trim().toLowerCase();
  if (!t.endsWith("?")) return false;   // no question mark, no doubt
  if (t.length > 90) return false;      // long enough to be a spec
  if (!/^(can|could|does|do|is|are|will|would|what|how|why|which|who|when)\b/.test(t)) {
    return false;
  }
  // A polite request names its thing -- "can you make me a tip calculator?"
  // Vague scope words mean they are asking about the tool, not ordering.
  const vague = /\b(any|anything|what kind|what sort|whatever|something)\b/.test(t);
  // ...but "app", "program", "thing" name nothing on their own. "Can you
  // make an app?" is a question about the product, and matching "an app"
  // as a named subject sent exactly that phrase into a fifteen-minute
  // build. Only generic when the sentence ENDS there: "can you make an app
  // that tracks my water intake?" says what it wants and must still build.
  const generic = /\b(a|an|the|any)\s+(app|application|program|thing|software|tool)s?\s*\?$/.test(t);
  const namesAThing = /\b(a|an|me|my)\s+[a-z]+/.test(t) && !vague && !generic;
  return !namesAThing;
}

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
  const s = state.session;
  if (s && !s.result && s.failedRequest && !state.planning) {
    $("prompt").value = "";
    if (/^\s*(resume|continue|keep going|go on|try again|retry|finish( it)?|build( it)?)\s*[.!]*\s*$/i.test(text)) {
      say("YOU", text);
      say("KRATE", "Resuming the same build.");
      return make(s.failedRequest, { silent: true });
    }
    say("YOU", text);
    say("KRATE", "Folding that in and building again.");
    return make(`${s.failedRequest}\n\n(After the stopped build, they added: ${text})`, { silent: true });
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
    $("loginBtn").classList.remove("hidden");
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

/* The link field copies too. Someone who sees a link expects clicking it
   to do something, and the confirmation belongs on the thing they clicked
   rather than in a note further up the page. */
$("detailCode").addEventListener("click", async () => {
  const app = state.cloudApp;
  if (!app || !app.url) return;
  const code = $("detailCode");
  const label = $("detailCodeText");
  try {
    await navigator.clipboard.writeText(app.url);
    const was = label.textContent;
    code.classList.add("copied");
    label.textContent = "Copied";
    setTimeout(() => {
      code.classList.remove("copied");
      // Only put the link back if the page is still showing the same app.
      if (state.cloudApp === app) label.textContent = was;
    }, 1400);
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
$("shareBtn").addEventListener("click", openSendSheet);
$("changeBtn").addEventListener("click", () => {
  // Change is the composer, not a fourth mystery: put the cursor where the
  // next sentence goes, and let the box answer visibly -- a focus alone
  // read as "the button does nothing" (seen live).
  const box = $("prompt");
  if (!box) return;
  box.disabled = false;
  box.placeholder = "Say what to change - it becomes v2…";
  box.focus();
  box.scrollIntoView({ block: "end", behavior: "smooth" });
  const shell = box.closest(".composer-box") || box.parentElement;
  if (shell) {
    shell.classList.remove("glow");
    void shell.offsetWidth;
    shell.classList.add("glow");
  }
});
$("sendCardBtn").addEventListener("click", sendCard);
$("sendLinkBtn").addEventListener("click", () => {
  $("sendSheet").classList.add("hidden");
  openPublishSheet();
});
$("sendWrapBtn").addEventListener("click", () => {
  $("sendWrapOs").classList.toggle("hidden");
});
document.querySelectorAll("#sendWrapOs [data-wrap]").forEach((b) => {
  b.addEventListener("click", async () => {
    const app = currentApp();
    if (!app) return;
    const os = b.dataset.wrap;
    b.disabled = true;
    $("sendNote").textContent = "Making the wrap…";
    try {
      const wrapPath = await invoke("make_wrap", { path: app.path, target: os });
      $("sendNote").textContent =
        wrapPath.split(/[\\/]/).pop() +
        " is in the folder that just opened. It installs Krate once on their " +
        (os === "mac" ? "Mac" : os === "windows" ? "Windows PC" : "Linux machine") +
        ", then opens this app -- the player is downloaded, never bundled.";
      try { await invoke("reveal", { path: wrapPath }); } catch (e) {}
    } catch (err) {
      $("sendNote").textContent = plainWords(err);
    }
    b.disabled = false;
  });
});
$("sendRawBtn").addEventListener("click", async () => {
  const app = currentApp();
  if (!app) return;
  try { await invoke("reveal", { path: app.path }); } catch (e) {}
});
$("pubGo").addEventListener("click", publishFromSheet);
$("pubShotPick").addEventListener("click", () => pickPublishImage("shot"));
$("pubIconPick").addEventListener("click", () => pickPublishImage("icon"));
$("infoBtn").addEventListener("click", showInfo);
{
  const g = $("galleryBtn");
  if (g) g.addEventListener("click", () => show("cloud"));
}
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
// Settings is a dock page now; the old titlebar gear became the theme
// switch. The sheet stays reachable from the page's own controls.
$("settingsBtn")?.addEventListener("click", openSettings);
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
  // One action on the first screen. The code path is the fallback for a
  // browser hand-off that is not going to arrive; it appears when the wait
  // says so, or the moment the browser path errors -- not as a second
  // button competing with the first before anything has gone wrong.
  clearTimeout(state.gateFallback);
  state.gateFallback = setTimeout(() => $("loginBtn").classList.remove("hidden"), 15000);
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
  tauri.event.listen("build-shot", (e) => onBuildShot(e.payload))
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

/* =======================================================================
 * The dock, the two new pages, and the updater.
 *
 * Added at the end so the file above is untouched: every existing view and
 * flow keeps working exactly as it did, and these listeners layer the new
 * navigation on top.
 * ===================================================================== */

/* ---- dock navigation --------------------------------------------------- */
/* The dock's avatar carries the person's initial from the moment the app
 * knows who they are -- it showed a bare dot until the profile page was
 * visited, which is a page most people never open. */
async function seedDockAvatar() {
  try {
    const account = await invoke("account_status");
    const name = account?.name || account?.login || "";
    if (!name) return;
    const solo = $("dockProfile");
    if (account.avatar_url && solo) {
      solo.innerHTML = `<img src="${account.avatar_url}" alt="" />`;
    } else {
      const initial = $("dockInitial");
      if (initial) initial.textContent = name.trim().charAt(0).toUpperCase();
    }
  } catch (e) { /* signed out: the dot is the honest answer */ }
}
seedDockAvatar();

document.querySelectorAll("#dock button[data-page]").forEach((button) => {
  button.addEventListener("click", () => {
    const page = button.dataset.page;
    if (page === "apps") { showView("apps"); loadAppsPage(); return; }
    if (page === "cloud") { openCloud(); return; }
    if (page === "settings") { showView("settings"); loadSettingsPage(); return; }
    showView("home");
  });
});

$("dockProfile")?.addEventListener("click", () => {
  showView("profile");
  loadProfilePage();
});

/* Scrolling drops the dock lower and makes it solid, then it settles back.
 * Bound per scroller because each page owns its own scroll container. */
(function bindDockScroll() {
  let idle = null;
  const attach = (el) => {
    if (!el || el.dataset.dockBound) return;
    el.dataset.dockBound = "1";
    el.addEventListener("scroll", () => {
      // Low and solid WHILE the page is moving; back to rest the moment it
      // stops, wherever it stopped. Tying the resting state to scrollTop
      // meant the dock stayed pinned down for as long as somebody was
      // scrolled -- which is most of the time on a full page.
      document.body.classList.add("scrolled");
      clearTimeout(idle);
      idle = setTimeout(() => document.body.classList.remove("scrolled"), 260);
    }, { passive: true });
  };
  document.querySelectorAll("[data-scroller]").forEach(attach);
  // The home view scrolls in its own element.
  attach(document.querySelector(".home"));
})();

/* Keep the pill aligned when the window resizes -- a label's box moves. */
window.addEventListener("resize", () => {
  const on = document.querySelector("#dock button.on");
  if (on) moveGlide(on);
});

/* ---- starter suggestions ---------------------------------------------- */
document.querySelectorAll("#suggList button[data-sugg]").forEach((button) => {
  button.addEventListener("click", () => {
    const box = $("homePrompt");
    if (!box) return;
    box.value = button.dataset.sugg;
    box.focus();
    box.dispatchEvent(new Event("input"));
  });
});

/* ---- your apps page ----------------------------------------------------
 * The grid moved off the home screen: home is the prompt and nothing else,
 * and a person's library is a place they go to, not a wall they scroll past
 * to reach the thing they came for. renderSessions still owns the cards, so
 * there is one list, not two that can disagree. */
async function loadAppsPage() {
  try {
    const sessions = await invoke("sessions_list");
    renderSessions(sessions || []);
    const made = (sessions || []).filter((s) => s.result && s.result.path).length;
    const count = $("appsCount");
    if (count) count.textContent = made === 1 ? "1 app" : `${made} apps`;
  } catch (e) { /* the page still renders its empty state */ }
}

/* ---- settings page ----------------------------------------------------- */
async function loadSettingsPage() {
  try {
    const settings = await invoke("settings_get");
    const dir = $("setOutDir");
    if (dir) dir.textContent = (settings.out_dir || "").replace(/^\/Users\/[^/]+/, "~");
    const agent = $("setAgent");
    if (agent) agent.textContent = settings.agent || "claude";
  } catch (e) { /* the page still renders without them */ }
  paintTerminalSetting();
  checkForUpdate();
}

$("setOutBtn")?.addEventListener("click", async () => {
  try {
    const dir = await invoke("pick_folder");
    if (!dir) return;
    state.outDir = dir;
    await invoke("settings_set", { settings: { out_dir: dir, agent: state.agent } });
    const el = $("setOutDir");
    if (el) el.textContent = dir.replace(/^\/Users\/[^/]+/, "~");
  } catch (e) { /* cancelled */ }
});

$("setAgentBtn")?.addEventListener("click", () => {
  // The agent picker already exists as a sheet; reuse it rather than
  // building a second one that can drift.
  const chip = $("builtByChip");
  if (chip) chip.click();
});

// Only the switches wired to something real respond. A control that
// flips and changes nothing is a lie told in one click.
document.querySelectorAll(".sw:not([data-soon])").forEach((sw) => {
  sw.addEventListener("click", () => sw.classList.toggle("on"));
});

/* ---- the updater -------------------------------------------------------
 * Four states, and the words matter as much as the mechanism: a person
 * being asked to restart an app deserves to know what they get, how long
 * it takes, and that their work survives.
 *
 * Downloading and installing are the Studio's job, not a browser's: the
 * old flow opened a download page and left the person to find the file,
 * mount it, and drag it over the running app. */
let updateInfo = null;

function updIcon(kind) {
  const el = $("updIc");
  if (!el) return;
  if (kind === "working") { el.innerHTML = '<span class="upd-spin"></span>'; return; }
  if (kind === "done") {
    el.innerHTML = '<svg width="17" height="17" viewBox="0 0 16 16" fill="none"><path d="M3.5 8.4l3 3 6-6.4" stroke="#6cf4d7" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>';
    return;
  }
  el.innerHTML = '<svg width="17" height="17" viewBox="0 0 16 16" fill="none"><path d="M8 11V3M8 3L4.6 6.4M8 3l3.4 3.4" stroke="#6291ff" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/><path d="M2.8 12.4h10.4" stroke="#6291ff" stroke-width="1.6" stroke-linecap="round"/></svg>';
}

async function checkForUpdate() {
  const card = $("upd");
  if (!card) return;
  card.classList.remove("ready", "done", "working");
  updIcon("working");
  $("updT").textContent = "Checking for updates…";
  $("updAct").innerHTML = "";

  let mine = "";
  try { mine = String(await invoke("studio_version")).replace(/^v/, ""); } catch (e) {}
  $("updCurrent").textContent = mine || "unknown";

  try {
    const res = await fetch("https://api.github.com/repos/incyashraj/krate/releases/latest");
    const rel = await res.json();
    const latest = String(rel.tag_name || "").replace(/^v/, "");
    const newer = latest && mine
      && latest.localeCompare(mine, undefined, { numeric: true }) > 0;

    if (!newer) {
      updIcon("done");
      $("updT").textContent = "You are on the newest version";
      $("updS").textContent = `Version ${mine} · checked just now`;
      $("updAct").innerHTML = '<button class="set-mini" id="updCheckAgain">Check again</button>';
      $("updCheckAgain").addEventListener("click", checkForUpdate);
      $("dockBadge")?.classList.remove("show");
      $("updateChip")?.classList.add("hidden");
      return;
    }

    updateInfo = { latest, notes: rel.body || "" };
    card.classList.add("ready");
    updIcon("ready");
    $("updT").textContent = `Version ${latest} is ready`;
    $("updS").textContent = `You have ${mine}`;
    $("updAct").innerHTML =
      '<button class="set-mini" id="updLater">Later</button>' +
      '<button class="set-mini go" id="updNow">Update now</button>';
    $("updNow").addEventListener("click", () => runUpdate(latest));
    $("updLater").addEventListener("click", () => {
      $("updT").textContent = "Update postponed";
      $("updS").textContent = "It stays here whenever you want it";
      $("updAct").innerHTML = '<button class="set-mini go" id="updUndo">Update now</button>';
      $("updUndo").addEventListener("click", () => runUpdate(latest));
    });
    $("dockBadge")?.classList.add("show");

    // Release notes, trimmed to the lines a person can act on.
    const list = $("updList");
    if (list) {
      const lines = (rel.body || "")
        .split("\n")
        .map((l) => l.replace(/^[-*]\s*/, "").trim())
        .filter((l) => l && !l.startsWith("#") && l.length < 120)
        .slice(0, 4);
      list.innerHTML = lines.length
        ? lines.map((l) => `<li>${escapeHtml(l)}</li>`).join("")
        : "<li>Fixes and improvements</li>";
    }
  } catch (e) {
    updIcon("ready");
    // Title first, then the body. Setting them in the other order left
    // "Checking for updates..." sitting above "No internet", which reads
    // as two different states at once.
    $("updT").textContent = "Could not check for updates";
    $("updS").textContent = "No internet, or GitHub is busy. Your Krate still works.";
    $("upd").classList.remove("ready", "working", "done");
    $("updAct").innerHTML = '<button class="set-mini" id="updRetry">Try again</button>';
    $("updRetry").addEventListener("click", checkForUpdate);
  }
}

async function runUpdate(latest) {
  const card = $("upd");
  card.classList.remove("ready");
  card.classList.add("working");
  updIcon("working");
  $("updT").textContent = `Downloading version ${latest}`;
  $("updS").textContent = "Krate keeps working while this happens";
  $("updAct").innerHTML = "";

  try {
    await invoke("install_update", { version: latest });
    card.classList.remove("working");
    card.classList.add("done");
    updIcon("done");
    $("updT").textContent = `Version ${latest} is ready to install`;
    $("updS").textContent = "Takes about five seconds. Your apps and history stay.";
    $("updAct").innerHTML = '<button class="set-mini go" id="updRestart">Restart Krate</button>';
    $("updRestart").addEventListener("click", () => invoke("restart_for_update").catch(() => {}));
  } catch (err) {
    // Falling back to the browser is honest, not a failure: the file is
    // real and the person can finish it by hand.
    card.classList.remove("working");
    updIcon("ready");
    $("updT").textContent = "Download it from your browser";
    $("updS").textContent = "The in-app update could not run on this machine.";
    $("updAct").innerHTML = '<button class="set-mini go" id="updOpen">Open the download</button>';
    $("updOpen").addEventListener("click", () => {
      const ua = navigator.userAgent;
      const file = ua.includes("Windows")
        ? `krate-studio-${latest}-windows-x64-setup.exe`
        : ua.includes("Linux") && !ua.includes("Android")
          ? `krate-studio-${latest}-linux-x86_64.AppImage`
          : `krate-studio-${latest}-universal.dmg`;
      invoke("open_external", {
        url: `https://github.com/incyashraj/krate/releases/download/v${latest}/${file}`,
      }).catch(() => {});
    });
  }
}

/* ---- profile page ------------------------------------------------------ */
async function loadProfilePage() {
  try {
    const account = await invoke("account_status");
    const name = account?.name || account?.login || "Signed in";
    $("profName").textContent = name;
    $("profMail").textContent = account?.login ? `@${account.login}` : "";
    const initial = (name || "?").trim().charAt(0).toUpperCase();
    $("profInitial").textContent = initial;
    const dockInitial = $("dockInitial");
    if (dockInitial) dockInitial.textContent = initial;
    if (account?.avatar_url) {
      $("profAv").innerHTML = `<img src="${account.avatar_url}" alt="" />`;
      const solo = $("dockProfile");
      if (solo) solo.innerHTML = `<img src="${account.avatar_url}" alt="" />`;
    }
    // GitHub is how publishing is authorised, so it is connected by
    // definition once somebody is signed in.
    $("connGhSub").textContent = account?.login
      ? `${account.login} · used for publishing`
      : "Used when you publish a link";
    $("connGhAct").innerHTML = account?.login
      ? '<span class="linked"><svg width="10" height="10" viewBox="0 0 10 10" fill="none"><path d="M2 5.2l2 2L8 3" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>Connected</span>'
      : '<button class="set-mini" data-connect="github">Connect</button>';
  } catch (e) { /* signed out; the page still renders */ }

  // The numbers are the person's own work, read from what the app already
  // knows: finished sessions are apps made, and the cloud listing filtered
  // to this account is what they published. No new backend command, so
  // nothing here can disagree with the rest of the UI.
  try {
    const sessions = await invoke("sessions_list");
    const made = (sessions || []).filter((s) => s.result && s.result.path);
    $("profMade").textContent = String(made.length);
    // result.size is a display string the build wrote ("24 KB", "1.2 MB"),
    // not a number -- summing it raw produced "NaN KB" on the profile.
    const kb = made.reduce((sum, s) => {
      const text = String((s.result && s.result.size) || "");
      const value = parseFloat(text);
      if (!Number.isFinite(value)) return sum;
      return sum + (/mb/i.test(text) ? value * 1024 : value);
    }, 0);
    $("profSize").textContent = kb >= 1024
      ? `${(kb / 1024).toFixed(1)} MB`
      : `${Math.round(kb)} KB`;
  } catch (e) {}
  try {
    const raw = await invoke("cloud_apps");
    const data = JSON.parse(raw || "{}");
    const login = ($("profMail").textContent || "").replace(/^@/, "");
    const mine = (data.apps || []).filter(
      (a) => login && a.meta && a.meta.author_login === login
    );
    { const el = $("profPub"); if (el) el.textContent = String(mine.length); }
  } catch (e) {}
}

document.querySelectorAll("button[data-connect]").forEach((button) => {
  button.addEventListener("click", () => {
    // Google and Apple sign-in ride the same browser hop GitHub uses; the
    // hub owns the flow, so the app only has to open the door.
    const provider = button.dataset.connect;
    invoke("open_external", {
      url: `https://hub.krate.tech/login/${provider}/start`,
    }).catch(() => {});
  });
});

$("profSignOut")?.addEventListener("click", async () => {
  try { await invoke("account_logout"); } catch (e) {}
  showView("gate");
});

function escapeHtml(text) {
  return String(text).replace(/[&<>"']/g, (c) => ({
    "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;",
  })[c]);
}


/* ---- the cloud's loading shell -----------------------------------------
 * "Reading Krate Cloud…" on an empty screen reads as a stall. Cards in
 * the shape of the real ones say the same thing without the doubt, and
 * they occupy the space the answer will land in, so nothing jumps. */
function showCloudSkeleton() {
  const grid = $("cloudGrid");
  if (!grid) return;
  grid.innerHTML = Array.from({ length: 8 })
    .map((_, i) => `<div class="skel-card" style="animation-delay:${i * 70}ms">
        <div class="skel-thumb"></div>
        <div class="skel-line"></div>
        <div class="skel-line short"></div>
      </div>`)
    .join("");
}


/* ---- theme ------------------------------------------------------------
 * The sun in the titlebar looked like a theme switch and did nothing.
 * Now it is one: it flips the app between the dark room it was designed
 * in and a light one, and remembers the choice. */
(function themeToggle() {
  const button = document.getElementById("themeBtn");
  if (!button) return;
  const saved = localStorage.getItem("krate-theme");
  if (saved === "light") document.body.classList.add("light");
  paintThemeIcon();

  button.addEventListener("click", () => {
    document.body.classList.toggle("light");
    localStorage.setItem(
      "krate-theme",
      document.body.classList.contains("light") ? "light" : "dark"
    );
    paintThemeIcon();
  });

  function paintThemeIcon() {
    const light = document.body.classList.contains("light");
    button.title = light ? "Switch to dark" : "Switch to light";
    button.innerHTML = light
      ? '<svg width="15" height="15" viewBox="0 0 16 16" fill="none"><path d="M13.4 9.4A5.6 5.6 0 016.6 2.6a5.6 5.6 0 106.8 6.8z" stroke="currentColor" stroke-width="1.35" stroke-linejoin="round"/></svg>'
      : '<svg width="15" height="15" viewBox="0 0 16 16" fill="none"><circle cx="8" cy="8" r="3.1" stroke="currentColor" stroke-width="1.35"/><path d="M8 1.4v1.7M8 12.9v1.7M1.4 8h1.7M12.9 8h1.7M3.4 3.4l1.2 1.2M11.4 11.4l1.2 1.2M12.6 3.4l-1.2 1.2M4.6 11.4l-1.2 1.2" stroke="currentColor" stroke-width="1.35" stroke-linecap="round"/></svg>';
  }
})();

/* =======================================================================
 * First run, the greeting, and the examples.
 * ===================================================================== */

/* The examples people actually see.
 *
 * Chosen against what this Studio has really built (the sessions in
 * ~/.krate/studio/builds) rather than invented: each one is a shape Krate
 * is known to do well, each names a concrete outcome instead of a
 * category, and together they cover the three reasons somebody opens
 * this -- keep something, do a small job, play. "A habit tracker" was
 * one of three that all sounded like homework. */
const EXAMPLES = [
  "a habit tracker that shows my streak going",
  "a tip splitter for dinners with friends",
  "a snake game I can play offline",
  "a countdown to a date that matters",
  "a photo booth that uses my webcam",
  "a shared grocery list my partner can edit too",
  "a voice memo recorder with playback",
  "a flashcard app for spanish words",
];

function pickExamples(n) {
  // Rotate rather than randomise: a person who reopens the app should not
  // feel the ground move, but the set should not be frozen forever either.
  const day = Math.floor(Date.now() / 86400000);
  const out = [];
  for (let i = 0; i < n; i += 1) {
    out.push(EXAMPLES[(day + i * 3) % EXAMPLES.length]);
  }
  return out;
}

function paintExamples() {
  const list = $("suggList");
  if (!list) return;
  const trend = '<span class="trend"><svg width="14" height="14" viewBox="0 0 16 16" fill="none" aria-hidden="true"><path d="M1.8 11.2L6 7l3 3 5.2-5.6" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></svg><path d="M10.6 4.4h3.8v3.8" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></span>';
  list.innerHTML = pickExamples(3)
    .map((text) => {
      const shown = text.charAt(0).toUpperCase() + text.slice(1);
      return `<button type="button" data-sugg="${text.replace(/"/g, "&quot;")}">
        <span class="trend"><svg width="14" height="14" viewBox="0 0 16 16" fill="none" aria-hidden="true"><path d="M1.8 11.2L6 7l3 3 5.2-5.6" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/><path d="M10.6 4.4h3.8v3.8" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></svg></span>
        ${shown}
        <svg class="go" width="14" height="14" viewBox="0 0 16 16" fill="none"><path d="M6 3.5L10.5 8 6 12.5" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></svg>
      </button>`;
    })
    .join("");
  list.querySelectorAll("button[data-sugg]").forEach((button) => {
    button.addEventListener("click", () => {
      const box = $("homePrompt");
      if (!box) return;
      box.value = button.dataset.sugg;
      box.focus();
      box.dispatchEvent(new Event("input"));
    });
  });
}

/* The greeting: "Hi Y" -- their first name's initial, from whichever
 * source knows it. The account's name wins because it is theirs; the
 * onboarding name is the fallback for anyone who skipped signing in. */
function paintGreeting() {
  const greet = $("homeGreet");
  const title = $("homeTitle") || document.querySelector(".home-title");
  const fromAccount = state.account && state.account.name;
  const saved = localStorage.getItem("krate-name") || "";
  const name = (fromAccount || saved || "").trim();
  const initial = name ? name.trim().charAt(0).toUpperCase() : "";
  // "What are we making, Y?" -- the greeting IS the headline, so the
  // screen asks one question instead of stacking a label above a slogan.
  // A small pill with the initial in it, then the greeting: at 15px of
  // faint grey the old line was there and unreadable, and a name is the
  // one thing on this screen that should feel addressed to a person.
  if (greet) {
    greet.innerHTML = initial
      ? `<span class="greet-badge">${initial}</span><span>Hi, ${
          (name.split(/\s+/)[0] || "").replace(/[<>]/g, "")
        }</span>`
      : "";
  }
  if (title) title.textContent = "Make an app you can actually send someone";
}

/* ---- onboarding -------------------------------------------------------- */
const ONBOARD_KEY = "krate-onboarded";

function needsOnboarding() {
  return !localStorage.getItem(ONBOARD_KEY);
}

function obGo(step) {
  document.querySelectorAll(".ob-scene").forEach((scene) => {
    const on = Number(scene.dataset.step) === step;
    scene.classList.toggle("on", on);
    if (on) {
      // Re-trigger the entrance: a scene that has already played would
      // otherwise appear fully formed, and the stagger is the point.
      scene.querySelectorAll(".ob-h, .ob-p, .ob-cta, .ob-agents, .ob-input, .ob-quiet, .fan-card")
        .forEach((el) => {
          el.style.animation = "none";
          void el.offsetWidth;
          el.style.animation = "";
        });
    }
  });
  if (step === 2) obLoadAgents();
}

async function obLoadAgents() {
  const box = $("obAgents");
  const next = $("obAgentNext");
  if (!box) return;
  try {
    const agents = await invoke("agents");
    const usable = (agents || []).filter((a) => a.state === "working");
    if (!agents || !agents.length) throw new Error("none");
    box.innerHTML = agents
      .map((a) => {
        const ok = a.state === "working";
        // "not installed" was said of every row that was not working,
        // including one that IS installed and only needs a sign-in. Telling
        // someone to install what they already have is how an AI picker
        // becomes a dead end (K-183).
        const label = ok
          ? "ready"
          : a.state === "not-ready"
            ? "needs signing in"
            : "not installed";
        return `<button class="ob-agent${ok ? "" : " off"}" data-agent="${a.name}" ${ok ? "" : "disabled"}>
          <span class="ob-agent-mark">${aiLogo(a.name)}</span>
          <span class="ob-agent-name">${a.label}</span>
          <span class="ob-agent-state">${label}</span>
        </button>`;
      })
      .join("");
    // Preselect the one that works, so the common case is one click.
    if (usable.length) {
      const first = box.querySelector(`[data-agent="${usable[0].name}"]`);
      if (first) first.classList.add("picked");
      state.agent = usable[0].name;
      if (next) next.disabled = false;
    }
    box.querySelectorAll("button[data-agent]").forEach((button) => {
      const info = (agents || []).find((x) => x.name === button.dataset.agent);
      if (info && info.state !== "working") {
        // An unavailable tool is not a dead row: it is one click from
        // being the tool they use.
        button.disabled = false;
        button.classList.remove("off");
        button.addEventListener("click", async () => {
          const label = button.querySelector(".ob-agent-state");
          if (label) label.textContent = "installing";
          try {
            if (info.state === "missing" && info.install_package) {
              await invoke("install_agent", { name: info.name });
            }
            await invoke("sign_in_agent", { name: info.name });
            if (label) label.textContent = "finish in terminal";
          } catch (err) {
            if (label) label.textContent = "could not install";
          }
        });
        return;
      }
      button.addEventListener("click", async () => {
        box.querySelectorAll("button").forEach((b) => b.classList.remove("picked"));
        button.classList.add("picked");
        state.agent = button.dataset.agent;
        if (next) next.disabled = false;
        try {
          const settings = await invoke("settings_get");
          await invoke("settings_set", {
            settings: { out_dir: settings.out_dir, agent: state.agent },
          });
        } catch (e) {}
      });
    });
    if (!usable.length) {
      // Honest, and actionable: name the tool and how to get it.
      const help = (agents || [])[0];
      box.innerHTML += `<p class="ob-none">No AI tool found yet. ${
        help && help.detail ? help.detail : "Install one, then come back — Krate will notice it."
      }</p>`;
      if (next) next.disabled = false;
    }
  } catch (e) {
    box.innerHTML = `<p class="ob-none">Krate could not check for AI tools.
      You can pick one later in Settings.</p>`;
    if (next) next.disabled = false;
  }
}


function finishOnboarding() {
  const name = ($("obName") && $("obName").value.trim()) || "";
  if (name) localStorage.setItem("krate-name", name);
  localStorage.setItem(ONBOARD_KEY, "1");
  showView("home");
  paintGreeting();
  paintExamples();
  // Land them on the prompt box with the cursor already in it: the last
  // thing onboarding promised was "make my first app".
  const box = $("homePrompt");
  if (box) setTimeout(() => box.focus(), 260);
}

// Bound by data-go, not by class: the rebuild renamed .ob-next to .ob-cta
// and the old selector matched nothing, so Start did nothing at all.
document.querySelectorAll("[data-go]").forEach((button) => {
  button.addEventListener("click", () => obGo(Number(button.dataset.go)));
});
$("obFinish")?.addEventListener("click", finishOnboarding);
$("obSkipAll")?.addEventListener("click", () => {
  localStorage.setItem(ONBOARD_KEY, "1");
  showView("home");
  paintGreeting();
  paintExamples();
});
$("obSignIn")?.addEventListener("click", () => {
  invoke("login_browser").catch(() => {});
});
$("obName")?.addEventListener("keydown", (e) => {
  if (e.key === "Enter") finishOnboarding();
});

paintExamples();
paintGreeting();

/* Signing in is not required to make an app -- only to publish one. The
 * gate offered no way past, so anyone who reached it signed out was stuck
 * behind an account wall guarding nothing. */
$("gateSkip")?.addEventListener("click", () => {
  localStorage.setItem(ONBOARD_KEY, "1");
  enterHome();
});

/* Replaying the welcome. Onboarding runs once, which makes it the hardest
 * screen in the app to look at again -- including for whoever is building
 * it. This is the honest way to see it, and it belongs in Settings rather
 * than in a console command only we would know. */
$("replayOnboard")?.addEventListener("click", () => {
  localStorage.removeItem(ONBOARD_KEY);
  showView("onboard");
  obGo(1);
});

/* ---- the `krate` command in a terminal (K-188) -------------------------
 *
 * A person who drags Krate.app into Applications has no `krate` on PATH:
 * the engine lives inside the bundle, and /usr/local/bin is root:wheel on a
 * stock Mac, so first-run setup could never make the symlink. Every support
 * instruction we give starts with a `krate` command, so this was a dead end
 * for exactly the people who most need help.
 *
 * A button, not a prompt on launch: being asked for a password by an app you
 * have just installed is worse than not having the shortcut.
 */
async function paintTerminalSetting() {
  const group = $("setTerminalGroup");
  if (!group) return;
  let info;
  try {
    info = await invoke("terminal_status");
  } catch {
    return;
  }
  if (!info || !info.supported) return;
  group.classList.remove("hidden");
  const hint = $("setTermHint");
  const btn = $("setTermBtn");
  if (info.linked) {
    hint.textContent = "Ready -- run `krate --version` in a terminal";
    btn.textContent = "Done";
    btn.disabled = true;
  } else {
    hint.textContent = "Not set up yet -- needs your password once";
    btn.textContent = "Set up";
    btn.disabled = false;
  }
}

$("setTermBtn")?.addEventListener("click", async () => {
  const btn = $("setTermBtn");
  const hint = $("setTermHint");
  btn.disabled = true;
  btn.textContent = "Asking…";
  try {
    hint.textContent = await invoke("link_terminal_tool");
    btn.textContent = "Done";
  } catch (err) {
    // A cancelled password box is a decision, not a fault.
    hint.textContent = String(err && err.message ? err.message : err);
    btn.textContent = "Set up";
    btn.disabled = false;
  }
});

/* ---- the AI picker's Refresh (K-194) -----------------------------------
 *
 * A readiness answer is cached for fifteen minutes, keyed on the tool's path
 * and mtime -- and signing in changes neither. Somebody who signed in to
 * Claude in a terminal and came straight back was told it still was not
 * ready, with no way to say "look again". */
$("aiRefresh")?.addEventListener("click", async () => {
  const btn = $("aiRefresh");
  const note = $("aiRefreshNote");
  btn.disabled = true;
  btn.textContent = "Checking…";
  note.textContent = "";
  try {
    state.agents = await invoke("refresh_agents");
    openAiSheet();
    const ready = (state.agents || []).filter((a) => a.state === "working").length;
    note.textContent = ready
      ? `${ready} ready`
      : "Still none ready -- follow a fix above, then press Refresh again.";
  } catch (err) {
    note.textContent = String(err && err.message ? err.message : err);
  }
  btn.disabled = false;
  btn.textContent = "Refresh";
});
