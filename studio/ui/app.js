/* Krate Studio, the front of it.
 *
 * One state machine: idle -> building -> done | failed. The backend is the
 * krate engine spawned by the Tauri shell; every line it prints streams in
 * as an event. In a plain browser (no Tauri) a mock backend stands in, so
 * the design can be seen and judged without building the shell -- and so a
 * broken engine never makes the window itself look broken.
 */

"use strict";

const tauri = window.__TAURI__ || null;
const $ = (id) => document.getElementById(id);

/* The five things a person watches happen. The engine prints many lines;
 * they collapse onto these so progress reads as a story, not a log. The
 * raw lines stay one click away under "details" -- honesty without noise. */
const STAGES = [
  { key: "think", label: "Understanding what you asked for" },
  { key: "write", label: "Writing the code" },
  { key: "build", label: "Building it" },
  { key: "pack",  label: "Packing it into one file" },
  { key: "wall",  label: "Checking it only touches what it declared" },
];

const state = {
  phase: "idle",
  request: "",
  history: [],          // every accepted request, oldest first
  startedAt: 0,
  timer: null,
  stageIndex: -1,
  result: null,         // { path, name, size, asks: [] }
};

/* ---- agent chip ------------------------------------------------------- */

async function refreshAgent() {
  try {
    const agents = tauri
      ? await tauri.core.invoke("agents")
      : [{ name: "claude", label: "Claude", state: "working" }];
    const best = agents.find((a) => a.state === "working") || agents[0];
    if (!best) {
      setAgent("bad", "no AI found", "Install Claude Code or another supported AI");
      return;
    }
    const dot = best.state === "working" ? "ok" : best.state === "not-ready" ? "warn" : "bad";
    const text =
      best.state === "working" ? best.label
      : best.state === "not-ready" ? `${best.label} · ${best.detail || "not ready"}`
      : `${best.label} · not installed`;
    setAgent(dot, text, best.detail || "");
    state.agent = best.name;
  } catch (err) {
    setAgent("bad", "engine not found", String(err));
  }
}

function setAgent(dot, text, title) {
  $("agentDot").className = `dot ${dot}`;
  $("agentName").textContent = text;
  $("agentChip").title = title || "";
}

/* ---- the thread ------------------------------------------------------- */

function say(who, body, cls = "") {
  $("threadEmpty")?.remove();
  const el = document.createElement("div");
  el.className = `msg ${cls}`;
  el.innerHTML = `<span class="who">${who}</span><span class="body"></span>`;
  el.querySelector(".body").textContent = body;
  $("thread").appendChild(el);
  $("thread").scrollTop = $("thread").scrollHeight;
  return el.querySelector(".body");
}

/* ---- phases ----------------------------------------------------------- */

function show(phase) {
  state.phase = phase;
  for (const id of ["stateIdle", "stateBuilding", "stateDone", "stateFailed"]) {
    $(id).classList.add("hidden");
  }
  $({ idle: "stateIdle", building: "stateBuilding", done: "stateDone", failed: "stateFailed" }[phase])
    .classList.remove("hidden");
}

function beginBuild(title) {
  $("buildTitle").textContent = title;
  $("stages").innerHTML = STAGES.map(
    (s) => `<li data-key="${s.key}"><span class="tick"></span>${s.label}</li>`,
  ).join("");
  $("buildLog").textContent = "";
  state.stageIndex = -1;
  advanceStage("think");
  state.startedAt = Date.now();
  clearInterval(state.timer);
  state.timer = setInterval(() => {
    const s = Math.floor((Date.now() - state.startedAt) / 1000);
    $("elapsed").textContent = `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
  }, 1000);
  show("building");
}

function advanceStage(key) {
  const idx = STAGES.findIndex((s) => s.key === key);
  if (idx <= state.stageIndex) return;
  state.stageIndex = idx;
  document.querySelectorAll("#stages li").forEach((li, i) => {
    li.className = i < idx ? "done" : i === idx ? "now" : "";
    li.querySelector(".tick").textContent = i < idx ? "✓" : "";
  });
}

/* Map the engine's own lines onto the stage story. These are our lines,
 * printed by our CLI -- if one changes, the worst case is a stage advancing
 * late, never a wrong claim. */
function onEngineLine(line) {
  const log = $("buildLog");
  log.textContent += line + "\n";
  log.scrollTop = log.scrollHeight;
  if (/writing the app|starter|agent|changing the app/i.test(line)) advanceStage("write");
  if (/==> building/.test(line)) advanceStage("build");
  if (/==> packing/.test(line)) advanceStage("pack");
  if (/==> verifying/.test(line)) advanceStage("wall");
}

function finishBuild(result) {
  clearInterval(state.timer);
  document.querySelectorAll("#stages li").forEach((li) => {
    li.className = "done";
    li.querySelector(".tick").textContent = "✓";
  });
  state.result = result;
  $("doneName").textContent = result.name;
  $("doneSize").textContent = result.size;
  $("asks").innerHTML = (result.asks || [])
    .map((a) => `<li>${friendlyAsk(a)}</li>`)
    .join("");
  $("shot").src = result.shot || "";
  $("shareResult").classList.add("hidden");
  const mins = Math.round((Date.now() - state.startedAt) / 60000);
  say("KRATE", `done · ${result.name} · ${result.size}${mins ? ` · ${mins} min` : ""}`, "krate");
  show("done");
  $("prompt").placeholder = "Want it different? Say what to change…";
  $("composerHint").textContent = "changes edit the app in place · still a few minutes, the AI reads before it edits";
}

function failBuild(why) {
  clearInterval(state.timer);
  /* The one hard rule of this card: plain words. A person here must never
   * meet a compiler error, an exit code, or a crate name. */
  $("failWhy").textContent = why;
  say("KRATE", "that build didn't come together", "krate");
  show("failed");
}

function friendlyAsk(cap) {
  const map = {
    "ui.window:create": "open a window",
    "io.stdout": "print text",
    "io.args": "read its start-up options",
    "store.kv": "save your data on this computer",
    "time.clock": "read the clock",
    "net.http": "reach the internet",
  };
  return map[cap] || cap;
}

/* ---- driving the engine ----------------------------------------------- */

async function make(request) {
  state.history.push(request);
  say("YOU", request);
  $("prompt").value = "";
  $("send").disabled = true;

  /* The first message makes the app. Every message after edits it in
   * place: the .krate carries its own source, so the engine hands the AI
   * the existing code and the change, and the diff is a few lines -- never
   * a from-scratch rebuild. */
  const revising = Boolean(state.result && state.result.path);
  beginBuild(revising ? "Making your change" : "Making your app");

  try {
    const result = !tauri
      ? await mockCreate()
      : revising
        ? await tauri.core.invoke("revise_app", { path: state.result.path, change: request })
        : await tauri.core.invoke("create_app", { request });
    finishBuild(result);
  } catch (err) {
    failBuild(plainWords(err));
  } finally {
    $("send").disabled = false;
  }
}

function plainWords(err) {
  const text = String(err && err.message ? err.message : err);
  if (/sign ?in|auth|logged/i.test(text)) return "Your AI needs signing in. Click the chip at the top left for how.";
  if (/quota|rate.?limit/i.test(text)) return "Your AI is out of quota right now. It usually comes back within the hour.";
  if (/network|offline|dns|connect/i.test(text)) return "The internet connection dropped mid-build.";
  if (/toolchain|rustup|cargo/i.test(text)) return "The build tools aren't set up yet. Open Krate once from the terminal, or try again to let it install them.";
  return "Something in the build went wrong. Trying again usually works; your words are kept.";
}

/* ---- done-card actions ------------------------------------------------ */

async function openApp() {
  if (tauri) await tauri.core.invoke("open_app", { path: state.result.path });
}
async function share() {
  $("shareBtn").disabled = true;
  $("shareBtn").textContent = "Publishing…";
  try {
    const url = tauri
      ? await tauri.core.invoke("publish", { path: state.result.path })
      : "https://hub.krate.tech/a/example";
    const el = $("shareResult");
    el.textContent = url + "  (copied)";
    el.classList.remove("hidden");
    try { await navigator.clipboard.writeText(url); } catch {}
  } catch (err) {
    $("shareResult").textContent = plainWords(err);
    $("shareResult").classList.remove("hidden");
  } finally {
    $("shareBtn").disabled = false;
    $("shareBtn").textContent = "Share";
  }
}
async function reveal() {
  if (tauri) await tauri.core.invoke("reveal", { path: state.result.path });
}

/* ---- mock backend: design-review mode only ---------------------------- */

async function mockCreate() {
  const lines = [
    "==> asking your AI to write the app",
    "  1. reading the request",
    "  7. writing src/lib.rs (agent)",
    "==> building the component",
    "==> packing habit-tracker.krate",
    "==> verifying the permission wall",
  ];
  for (const l of lines) {
    onEngineLine(l);
    await new Promise((r) => setTimeout(r, 700));
  }
  return {
    path: "/tmp/habit-tracker.krate",
    name: "habit-tracker.krate",
    size: "24 KB",
    asks: ["ui.window:create", "store.kv", "io.args"],
    shot: mockShot(),
  };
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

/* ---- wiring ----------------------------------------------------------- */

function submit() {
  const text = $("prompt").value.trim();
  if (!text || state.phase === "building") return;
  make(text);
}

$("send").addEventListener("click", submit);
$("prompt").addEventListener("keydown", (e) => {
  if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) { e.preventDefault(); submit(); }
});
$("ideas")?.addEventListener("click", (e) => {
  if (e.target.classList.contains("idea")) { $("prompt").value = e.target.textContent; submit(); }
});
$("openBtn").addEventListener("click", openApp);
$("shareBtn").addEventListener("click", share);
$("revealBtn").addEventListener("click", reveal);
$("retryBtn").addEventListener("click", () => {
  const last = state.history.pop();
  $("prompt").value = last || "";
  show("idle");
  submit();
});

if (tauri) {
  tauri.event.listen("engine-line", (e) => onEngineLine(e.payload));
}
refreshAgent();
