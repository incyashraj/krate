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
  for (const id of ["viewGate", "viewHome", "viewSession", "viewCloud"]) {
    $(id).classList.add("hidden");
  }
  const view = $({
    gate: "viewGate", home: "viewHome", session: "viewSession", cloud: "viewCloud",
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
    $("gateError").textContent = plainWords(err);
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
    $("gateUrl").textContent = step.url;
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
  refreshAgents();
  const settings = await invoke("settings_get");
  state.outDir = settings.out_dir;
  state.agent = settings.agent || "claude";
  renderSessions(await invoke("sessions_list"));
  renderBuilding();
}

/* A build keeps running while you browse. Without this the home screen looked
 * idle, the next request was refused with "one app is already being made",
 * and there was no way to reach the build to stop it. */
function renderBuilding() {
  const bar = $("buildingNow");
  if (!bar) return;
  const live = state.phase === "building" && state.buildingSession;
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
  state.session = s;
  state.attachments = [];
  $("railTitle").textContent = s.title;
  $("thread").innerHTML = "";
  for (const m of s.messages) {
    appendMessage(m.who, m.body, m.files);
  }
  if (s.result) {
    fillDone(s.result, { reveal: false });
    show("done");
    setRevisePlaceholders();
  } else {
    show("idle");
  }
  showView("session");
  $("prompt").focus();
}

async function persist() {
  const s = state.session;
  if (!s) return;
  s.updated = Math.floor(Date.now() / 1000);
  try {
    await invoke("session_save", { session: s });
  } catch (e) {
    /* history is a convenience; never let saving break making */
  }
}

/* ---- thread ----------------------------------------------------------- */

function appendMessage(who, body, files) {
  const el = document.createElement("div");
  el.className = `msg ${who === "KRATE" ? "krate" : ""}`;
  el.innerHTML = `<span class="who">${who}</span><span class="body"></span>`;
  el.querySelector(".body").textContent = body;
  if (files && files.length) {
    const f = document.createElement("span");
    f.className = "files";
    f.textContent = `📎 ${files.map(baseName).join(", ")}`;
    el.appendChild(f);
  }
  $("thread").appendChild(el);
  $("thread").scrollTop = $("thread").scrollHeight;
}

function say(who, body, files) {
  appendMessage(who, body, files);
  state.session.messages.push({ who, body, files: files || [], when: Math.floor(Date.now() / 1000) });
}

const baseName = (p) => p.split(/[\\/]/).pop();

/* ---- build state machine ---------------------------------------------- */

function show(phase) {
  state.phase = phase;
  for (const id of ["stateIdle", "stateBuilding", "stateDone", "stateFailed"]) {
    $(id).classList.add("hidden");
  }
  $({ idle: "stateIdle", building: "stateBuilding", done: "stateDone", failed: "stateFailed" }[phase])
    .classList.remove("hidden");
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

function advanceStage(key) {
  const idx = STAGES.findIndex((s) => s.key === key);
  if (idx <= state.stageIndex) return;
  state.stageIndex = idx;
  // Two milestones worth saying out loud. Not five -- a chat that narrates
  // every step is noise, and the stage list already shows all of them.
  if (STAGE_SAID[key] && state.session) say("KRATE", STAGE_SAID[key]);
  document.querySelectorAll("#stages li").forEach((li, i) => {
    li.className = i < idx ? "done" : i === idx ? "now" : "";
  });
  // One line at eye level says where we are; the full list stays one click
  // away for anyone who wants it.
  const nowStage = $("nowStage");
  if (nowStage) nowStage.textContent = STAGES[idx].label;
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
  const log = $("buildLog");
  log.textContent += line + "\n";
  log.scrollTop = log.scrollHeight;
  const clean = line.replace(/^=+>\s*/, "").trim();
  if (clean) {
    $("nowLine").textContent = clean;
    state.lastLineAt = Date.now();
  }
  if (/authoring|writing (the|your) app|starter|asking|agent|changing the app/i.test(line)) advanceStage("write");
  if (/==> building|Compiling|Generating bindings/i.test(line)) advanceStage("build");
  if (/==> packing/.test(line)) advanceStage("pack");
  if (/==> verifying/.test(line)) advanceStage("wall");
}

/* When the engine goes quiet -- an AI thinking is real silence -- the
 * heartbeat keeps beating with honest words, so quiet never looks dead. */
const THINKING = [
  "the AI is reading and thinking — this part is quiet",
  "still working — the writing shows up here when it starts",
  "big thoughts take a minute or two",
  "still at it — nothing is stuck",
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
  clearInterval(state.timer);
  document.querySelectorAll("#stages li").forEach((li) => {
    li.className = "done";
  });
  setProgress(1);
  const fill = $("buildFill");
  if (fill) fill.style.width = "100%";
  state.session.result = result;
  fillDone(result, { reveal: true });
  const mins = Math.round((Date.now() - state.startedAt) / 60000);
  say("KRATE", `Done — ${result.name}, ${result.size}${mins ? `, ${mins} min` : ""}. ` +
    `Open it on the right, or tell me what to change.`);
  show("done");
  setRevisePlaceholders();
  persist();
}

function setRevisePlaceholders() {
  $("prompt").placeholder = "Want it different? Say what to change…";
  $("composerHint").textContent = "changes edit the app in place · a few minutes, the AI reads before it edits";
}

function failBuild(why, request) {
  clearInterval(state.timer);
  state.lastFailed = request;
  /* The one hard rule of this card: plain words. A person here must never
   * meet a compiler error, an exit code, or a crate name. */
  if (why === "stopped") {
    $("failTitle").textContent = "Stopped.";
    $("failWhy").textContent = "Nothing was lost -- your words are kept, ready to send again.";
    say("KRATE", "stopped");
  } else {
    $("failTitle").textContent = "That one didn't come together.";
    $("failWhy").textContent = why;
    say("KRATE", "that build didn't come together");
  }
  show("failed");
  persist();
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
  // Two builds at once would leave the first unstoppable; the backend
  // refuses it too, but stopping here keeps the UI honest.
  if (state.phase === "building") return;
  if (!state.session) newSession(request);
  const files = state.attachments.slice();
  state.attachments = [];
  renderAttachments();
  say("YOU", request, files);
  persist();
  $("prompt").value = "";
  // The composer stays live during a build so a thought can be queued
  // rather than lost.
  $("prompt").placeholder = "Add a change — it runs when this finishes…";

  state.buildingSession = state.session;
  const revising = Boolean(currentApp());
  // The rail is a conversation: it should answer. Without this the left
  // side showed one line and then nothing for six minutes while the right
  // side did all the talking.
  say("KRATE", revising
    ? "Reading your app, then making that change."
    : "On it. I'll show you each step as it happens.");
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
    failBuild(plainWords(err), request);
  } finally {
    state.buildingSession = null;
    $("send").disabled = false;
    const queued = state.queued;
    state.queued = null;
    if (queued) {
      setRevisePlaceholders();
      setTimeout(() => make(queued), 400);
    }
  }
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
  if (/sign ?in|auth|logged/i.test(text)) return "Your AI needs signing in. Click its name at the top for the fix.";
  if (/quota|rate.?limit/i.test(text)) return "Your AI is out of quota right now. It usually comes back within the hour.";
  if (/network|offline|dns|connect/i.test(text)) return "The internet connection dropped mid-build.";
  if (/toolchain|rustup|cargo/i.test(text)) return "The build tools aren't set up yet. Trying again lets Krate install them.";
  return "Something in the build went wrong. Trying again usually works; your words are kept.";
}

/* ---- agents ----------------------------------------------------------- */

async function refreshAgents() {
  try {
    state.agents = await invoke("agents");
  } catch (err) {
    setChips("bad", "engine not found", String(err));
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
  if (chosen.state === "working") state.agent = chosen.name;
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
}

function openAiSheet() {
  const list = $("aiList");
  list.innerHTML = "";
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
    }
    list.appendChild(row);
  }
  $("aiSheet").classList.remove("hidden");
}

/* ---- attachments ------------------------------------------------------ */

async function attach() {
  const picked = await invoke("pick_files");
  for (const p of picked) {
    if (!state.attachments.includes(p)) state.attachments.push(p);
  }
  renderAttachments();
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

async function openCloud() {
  showView("cloud");
  $("cloudError").classList.add("hidden");
  $("cloudLoading").classList.remove("hidden");
  $("cloudGrid").innerHTML = "";
  try {
    const payload = JSON.parse(await invoke("cloud_apps"));
    renderCloud(payload.apps || []);
  } catch (err) {
    $("cloudError").textContent = String(err);
    $("cloudError").classList.remove("hidden");
  } finally {
    $("cloudLoading").classList.add("hidden");
  }
}

function renderCloud(apps) {
  const grid = $("cloudGrid");
  grid.innerHTML = "";
  if (!apps.length) {
    $("cloudError").textContent = "Nothing published yet. Yours could be first.";
    $("cloudError").classList.remove("hidden");
    return;
  }
  for (const app of apps) {
    const meta = app.meta || {};
    const card = document.createElement("button");
    card.className = "cloud-card";

    const name = document.createElement("p");
    name.className = "cloud-name";
    name.textContent = meta.name || "Untitled app";
    card.appendChild(name);

    if (meta.author) {
      const by = document.createElement("p");
      by.className = "cloud-by";
      // The avatar is remote, and the page is otherwise offline-only. A
      // broken image would look like a bug, so it simply removes itself.
      if (meta.avatar_url) {
        const img = document.createElement("img");
        img.src = meta.avatar_url;
        img.alt = "";
        img.onerror = () => img.remove();
        by.appendChild(img);
      }
      by.appendChild(document.createTextNode(meta.author));
      card.appendChild(by);
    }

    const bits = [];
    if (meta.size) bits.push(`${Math.round(meta.size / 1024)} KB`);
    if (meta.published) bits.push(timeAgo(meta.published));
    if (bits.length) {
      const line = document.createElement("p");
      line.className = "cloud-meta";
      line.textContent = bits.join(" \u00b7 ");
      card.appendChild(line);
    }

    const go = document.createElement("p");
    go.className = "cloud-open";
    go.textContent = "Open it \u2192";
    card.appendChild(go);

    card.addEventListener("click", async () => {
      go.textContent = "Opening\u2026";
      try {
        await invoke("cloud_run", { url: app.url });
        go.textContent = "Open it \u2192";
      } catch (err) {
        go.textContent = String(err);
      }
    });
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
async function share() {
  $("shareBtn").disabled = true;
  $("shareBtn").textContent = "Publishing…";
  try {
    const app = currentApp();
    if (!app) throw new Error("there is no finished app to share yet");
    const url = await invoke("publish", { path: app.path });
    state.session.result.share_url = url;
    $("shareLink").textContent = url;
    $("shareResult").classList.remove("hidden", "error");
    let copied = false;
    try { await navigator.clipboard.writeText(url); copied = true; } catch (e) {}
    $("shareCopied").classList.toggle("hidden", !copied);
    persist();
  } catch (err) {
    $("shareLink").textContent = plainWords(err);
    $("shareCopied").classList.add("hidden");
    $("shareResult").classList.remove("hidden");
    $("shareResult").classList.add("error");
  } finally {
    $("shareBtn").disabled = false;
    $("shareBtn").textContent = "Share";
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
  // The gradient is a single ::after layer that repeats the word and masks
  // itself to the glyphs; it only needs to know what the word says.
  word.dataset.word = [...word.children].map((c) => c.textContent).join("");
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
  if (state.phase === "building") {
    // Do not drop it. A thought that arrives mid-build is exactly the
    // thought worth keeping -- queue it, say so, and run it when this
    // build finishes.
    state.queued = text;
    $("prompt").value = "";
    say("YOU", text);
    say("KRATE", "Noted — I'll do that as soon as this one is finished.");
    $("composerHint").textContent = "queued · runs when this build finishes";
    return;
  }
  make(text);
}

$("loginBtn").addEventListener("click", login);
$("homeSend").addEventListener("click", startFromHome);
$("homePrompt").addEventListener("keydown", (e) => {
  if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) { e.preventDefault(); startFromHome(); }
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
  if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) { e.preventDefault(); submitInSession(); }
});
$("backBtn").addEventListener("click", async () => {
  await persist();
  enterHome();
});
$("attachBtn").addEventListener("click", attach);
$("homeAttachBtn").addEventListener("click", attach);
$("cloudBtn").addEventListener("click", openCloud);
$("cloudBackBtn").addEventListener("click", enterHome);
$("cloudRefresh").addEventListener("click", openCloud);
$("stopBtn").addEventListener("click", () => invoke("stop_build"));
$("openBtn").addEventListener("click", openApp);
$("shareBtn").addEventListener("click", share);
$("revealBtn").addEventListener("click", async () => {
  const app = currentApp();
  if (!app) return;
  try {
    await invoke("reveal", { path: app.path });
  } catch (err) {
    showActionError(err);
  }
});
$("retryBtn").addEventListener("click", () => {
  const again = state.lastFailed;
  show("idle");
  if (again) make(again);
});
["agentChip", "agentChip2"].forEach((id) => {
  const chip = $(id);
  if (chip) chip.addEventListener("click", openAiSheet);
});
$("buildingNow")?.addEventListener("click", () => {
  if (state.buildingSession) openSession(state.buildingSession);
});
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

if (tauri) {
  // A rejected listen was invisible once: the capabilities grant was
  // missing, invokes worked, and the build screen froze on stage one with
  // an empty log. Surfacing the rejection into the log is the tripwire.
  tauri.event.listen("engine-line", (e) => onEngineLine(e.payload))
    .catch((err) => onEngineLine(`(!) event channel failed: ${err}`));
  tauri.event.listen("login-step", (e) => onLoginStep(e.payload))
    .catch(() => {});
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
      for (const l of lines) { onEngineLine(l); await sleep(600); }
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
