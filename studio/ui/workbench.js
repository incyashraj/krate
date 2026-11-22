/* Krate Studio's workbench (IC-995): beside the code, the app on the screens
 * it must fit, what it asks for as switches, and the checks it must pass --
 * in the session view's look.
 *
 * Devices: the app's first frame at the sizes check-app judges (a phone, a
 * tablet, a desktop window), one at a time or side by side, rebuilt on save.
 * Permissions: every ask in plain words, as a switch; turning one off opens
 * the app again without it, so you see what a person who says no sees.
 * Checks: check-app's stages and the app's own krate-check.toml task as
 * cards, in the order they ran.
 *
 * Desktop only, like the IDE (ide.js takes the IDE away on the web). Loaded
 * after ide.js; everything is inside one function because app.js and
 * bridge.js share one global scope in a tab (scope-test.mjs). It hears the
 * IDE through two events, kr-ide-open and kr-ide-built, and calls three
 * commands in studio/src/ide.rs: ide_devices, ide_asks, ide_checks.
 */
(function () {
  "use strict";
  const $ = (id) => document.getElementById(id);
  const desktop = (() => { try { return !!tauri; } catch (e) { return false; } })();
  if (!desktop || !$("wbStage")) return;
  const call = (cmd, args) => invoke(cmd, args);
  const esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]));
  const cap1 = (s) => String(s || "").replace(/^./, (c) => c.toUpperCase());

  // The sizes check-app's screens stage judges (FIT_SCREENS in
  // crates/runtime/src/usability.rs): what Devices shows is what is checked.
  const DEVICES = {
    phone: { name: "Phone", width: 390, height: 844 },
    tablet: { name: "Tablet", width: 820, height: 1180 },
    desktop: { name: "Desktop", width: 1280, height: 800 },
  };

  let path = null;      // the project the IDE has open
  let built = false;    // whether it has a good build to show
  let device = "window";
  let asks = [];        // [{cap, plain, rationale, required}]
  let off = new Set();  // asks the person turned off on the Permissions tab
  let seq = 0;          // the newest request wins; an older answer is dropped

  /* ---- tabs ------------------------------------------------------------ */
  function tab(name) {
    document.querySelectorAll(".wb-tabs [data-wb]").forEach((b) => {
      const on = b.dataset.wb === name;
      b.classList.toggle("on", on);
      b.setAttribute("aria-selected", on ? "true" : "false");
    });
    document.querySelectorAll(".wb [data-wbp]").forEach((p) => p.classList.toggle("hidden", p.dataset.wbp !== name));
    if (name === "perm" && built && !asks.length) loadAsks();
  }
  document.querySelectorAll(".wb-tabs [data-wb]").forEach((b) => b.addEventListener("click", () => tab(b.dataset.wb)));

  /* ---- devices --------------------------------------------------------- */
  function pickDevice(name) {
    device = name;
    document.querySelectorAll("#wbDevs [data-dev]").forEach((b) => {
      const on = b.dataset.dev === name;
      b.classList.toggle("on", on);
      b.setAttribute("aria-checked", on ? "true" : "false");
    });
    const own = name === "window";
    $("wbWindow").classList.toggle("hidden", !own);
    $("wbShots").classList.toggle("hidden", own);
    $("wbDevNote").innerHTML = own
      ? "The app's own first frame, from a real headless run. <b>Run</b> opens it for real."
      : name === "all"
        ? "The sizes Krate checks every app at before it is shared."
        : `${DEVICES[name].name}, ${DEVICES[name].width} × ${DEVICES[name].height}: the size Krate checks it at.`;
    if (!own) shoot();
  }
  document.querySelectorAll("#wbDevs [data-dev]").forEach((b) => b.addEventListener("click", () => pickDevice(b.dataset.dev)));

  function frame(d, inner, extra) {
    const portrait = d.height > d.width;
    return `<figure class="wb-dev ${portrait ? "tall" : "wide"}${extra ? " " + extra : ""}" style="--ar:${d.width}/${d.height};--w:${d.width}">
      <div class="wb-glass">${inner}</div>
      <figcaption>${esc(d.name)} <span>${d.width} × ${d.height}</span></figcaption>
    </figure>`;
  }
  function placeholders(list, words) {
    return list.map((d) => frame(d, `<div class="wb-wait"><i></i><span>${esc(words)}</span></div>`, "waiting")).join("");
  }
  // What is turned on right now: everything the app asks for, less what the
  // person switched off. Before the asks are known, None (all it declares).
  const grants = () => (asks.length ? asks.filter((a) => !off.has(a.cap)).map((a) => a.cap) : null);

  async function shoot(target) {
    const box = target || $("wbShots");
    if (!path) return;
    const list = target ? [DEVICES.phone] : device === "all" ? [DEVICES.phone, DEVICES.tablet, DEVICES.desktop] : [DEVICES[device]];
    box.classList.toggle("all", !target && device === "all");
    if (!built) { box.innerHTML = `<p class="ip-none">Build it to see it here.</p>`; return; }
    // The newest request for THIS picture wins: the device strip and the
    // Permissions reaction are separate pictures and must not cancel each
    // other.
    const mine = ++seq;
    box.dataset.seq = String(mine);
    const forPath = path;
    const stale = () => box.dataset.seq !== String(mine) || !box.isConnected || path !== forPath;
    box.innerHTML = placeholders(list, "Opening…");
    try {
      const shots = await call("ide_devices", { path, devices: list, grants: grants() });
      if (stale()) return;
      box.innerHTML = shots.map((s) => frame(s, s.shot
        ? `<img src="${s.shot}" alt="${esc(s.name)}: the app's first frame">`
        : `<div class="wb-no"><b>It did not open</b><pre>${esc(s.error || "")}</pre></div>`, s.shot ? "" : "bad")).join("");
    } catch (err) {
      if (stale()) return;
      box.innerHTML = `<div class="wb-no"><b>Could not show it</b><pre>${esc(err)}</pre></div>`;
    }
  }

  /* ---- permissions ----------------------------------------------------- */
  async function loadAsks() {
    if (!path) return;
    try {
      asks = await call("ide_asks", { path });
      off = new Set([...off].filter((c) => asks.some((a) => a.cap === c)));
      paintAsks();
    } catch (err) {
      $("wbPerms").innerHTML = `<p class="ip-none">${esc(err)}</p>`;
    }
  }
  function paintAsks() {
    const box = $("wbPerms");
    if (!asks.length) { box.innerHTML = `<p class="ip-none">It asks for nothing beyond a window.</p>`; return; }
    box.innerHTML = asks.map((a, i) => `
      <div class="wb-perm${off.has(a.cap) ? " is-off" : ""}">
        <div class="wb-perm-t"><b>${esc(cap1(a.plain))}</b>${a.required ? `<em>required</em>` : ""}
          <p>${esc(a.rationale || "")}</p><code>${esc(a.cap)}</code></div>
        <button type="button" class="sw${off.has(a.cap) ? "" : " on"}" role="switch" data-i="${i}" aria-checked="${off.has(a.cap) ? "false" : "true"}" aria-label="${esc(cap1(a.plain))}"></button>
      </div>`).join("");
    box.querySelectorAll(".sw").forEach((sw) => sw.addEventListener("click", () => {
      const a = asks[Number(sw.dataset.i)];
      if (off.has(a.cap)) off.delete(a.cap); else off.add(a.cap);
      paintAsks();
      reactShot();
    }));
  }
  // The app opened again with the switches as they are, on a phone: the
  // picture under the list is what a person who said no sees.
  function reactShot() {
    const box = $("wbPermShot");
    if (!off.size) { box.classList.add("hidden"); box.innerHTML = ""; if (device !== "window") shoot(); return; }
    box.classList.remove("hidden");
    const missing = asks.filter((a) => off.has(a.cap));
    const need = missing.filter((a) => a.required);
    box.innerHTML = `<p class="wb-said">${need.length
      ? `Without “${esc(need.map((a) => a.plain).join("”, “"))}” the app does not open: Krate asks for it first, and a person who says no is told why.`
      : `Opened without “${esc(missing.map((a) => a.plain).join("”, “"))}”. Whatever uses it now gets a polite refusal.`}</p><div class="wb-permdev"></div>`;
    shoot(box.querySelector(".wb-permdev"));
    if (device !== "window") shoot();
  }

  /* ---- checks ---------------------------------------------------------- */
  // check-app's stage names, in the words a person reads.
  const STAGES = {
    layout: ["The project", "Cargo.toml, the source and the manifest are where Krate looks."],
    manifest: ["The manifest", "Every permission is one Krate knows, with a reason."],
    build: ["It builds", "Compiled to a Krate app."],
    imports: ["Only Krate's API", "It reaches the computer only through what it declared."],
    run: ["It runs", "Opened once, headless, and drew its window."],
    usability: ["A person can use it", "Pressed, typed into and resized the way a person would."],
    task: ["Its own task", "The steps in krate-check.toml, done and checked."],
    screens: ["It fits every screen", "A phone, a tablet and a desktop window."],
    phone: ["It runs as a phone runs it", "On the interpreter iOS requires, first frame and scrolling timed."],
  };
  const words = (s) => STAGES[s] || [cap1(String(s).replace(/[-_]/g, " ")), ""];
  function card(state, stage, body, i) {
    const [title, what] = words(stage);
    return `<li class="wb-card ${state}" style="--i:${i}"><span class="wb-mark"></span>
      <div><b>${esc(title)}</b><p>${body || esc(what)}</p></div></li>`;
  }
  $("wbCheck").addEventListener("click", runChecks);
  async function runChecks() {
    if (!path) return;
    const btn = $("wbCheck"), list = $("wbCards");
    btn.disabled = true; btn.textContent = "Checking…";
    list.innerHTML = Object.keys(STAGES).slice(0, 3).map((s, i) => card("wait", s, "", i)).join("");
    try {
      const v = await call("ide_checks", { path });
      const passed = Array.isArray(v.stages) ? v.stages : [];
      let html = passed.map((s, i) => card("ok", s, "", i)).join("");
      const all = [].concat(v.usability_notes || []);
      // The app's own task passing is a pass, with its name, not a note.
      const task = all.map((n) => /^completes its task: (.+)$/.exec(n)).find(Boolean);
      if (task) html += card("ok", "task", esc(cap1(task[1])), passed.length);
      const notes = all.filter((n) => !/^completes its task: /.test(n) && !/^(responds to a press|answers the keyboard): not checked/.test(n));
      if (v.ok) {
        if (notes.length) html += `<li class="wb-card note" style="--i:${passed.length + 1}"><span class="wb-mark"></span><div><b>Worth a look</b>${notes.map((n) => `<p>${esc(n)}</p>`).join("")}</div></li>`;
        btn.textContent = "Passed · run again";
      } else {
        const fix = v.fix ? `<pre>${esc(v.fix)}</pre>` : "";
        html += card("bad", v.stage || "check", `${esc(v.detail || "It stopped here.")}${fix}`, passed.length);
        btn.textContent = "Run checks again";
      }
      list.innerHTML = html;
    } catch (err) {
      list.innerHTML = card("bad", "check", esc(err), 0);
      btn.textContent = "Run checks again";
    } finally {
      btn.disabled = false;
    }
  }

  /* ---- hardware: a pretend label printer ----------------------------- */
  // The app runs in its own window; what it prints lands in a folder Krate
  // prints into instead of a printer, and each job is drawn here as the
  // label would come out (labels.js: ZPL with Code 128 and QR, ESC/POS
  // receipts as text, a PNG as itself).
  let printing = false, poll = null, shownJobs = 0;
  const b64bytes = (b64) => Uint8Array.from(atob(b64 || ""), (c) => c.charCodeAt(0));
  function jobCard(job) {
    const li = document.createElement("li");
    li.className = "wb-job";
    li.style.setProperty("--i", "0");
    const head = `<div class="wb-job-h"><b>${esc(job.title || "Untitled job")}</b><span>${job.copies > 1 ? job.copies + " copies · " : ""}${esc(job.kind.toUpperCase())} · ${job.bytes.toLocaleString()} bytes</span></div>`;
    const bytes = b64bytes(job.data);
    const text = new TextDecoder("latin1").decode(bytes);
    const L = window.krLabels;
    if (job.kind === "raw" && L && L.looksLikeZpl(text)) {
      li.innerHTML = head + `<div class="wb-labels"></div>`;
      const box = li.querySelector(".wb-labels");
      const unknown = new Set();
      L.zplLabels(text).forEach((body) => {
        const r = L.drawZpl(body, { maxWidth: 300 });
        r.unknown.forEach((u) => unknown.add(u));
        const fig = document.createElement("div");
        fig.className = "wb-label";
        fig.appendChild(r.canvas);
        box.appendChild(fig);
      });
      if (unknown.size) li.insertAdjacentHTML("beforeend", `<p class="wb-job-note">Not drawn here: ${esc([...unknown].join(" "))}. The printer reads them; this preview does not.</p>`);
    } else if (job.kind === "raw" && L) {
      li.innerHTML = head + `<pre class="wb-receipt">${esc(L.escposText(bytes) || "(no text)")}</pre>`;
    } else if (job.kind === "png") {
      li.innerHTML = head + `<div class="wb-labels"><div class="wb-label"><img alt="${esc(job.title)}" src="data:image/png;base64,${job.data}"></div></div>`;
    } else {
      li.innerHTML = head + `<p class="wb-job-note">A ${esc(job.kind.toUpperCase())} document, sent as the printer would get it.</p>`;
    }
    return li;
  }
  async function pollJobs() {
    if (!path) return;
    try {
      const r = await call("ide_hw_jobs", { path });
      const jobs = r.jobs || [];
      const list = $("wbJobs");
      for (const job of jobs.slice(shownJobs)) list.prepend(jobCard(job));
      shownJobs = Math.max(shownJobs, jobs.length);
      if (printing && !r.running) setPrinting(false, jobs.length ? `The app closed after ${jobs.length === 1 ? "one job" : jobs.length + " jobs"}.` : "The app closed without printing.");
      else if (printing) $("wbPrintSt").textContent = jobs.length ? `Running · ${jobs.length === 1 ? "1 job" : jobs.length + " jobs"} printed, newest first.` : "Running · print something from the app's window.";
    } catch (err) { /* the next tick tries again */ }
  }
  function setPrinting(on, words) {
    printing = on;
    $("wbPrintRun").textContent = on ? "Stop" : "Run with it";
    $("wbPrinter").classList.toggle("on", on);
    if (words) $("wbPrintSt").textContent = words;
    clearInterval(poll); poll = null;
    if (on) poll = setInterval(pollJobs, 1000);
  }
  $("wbPrintRun").addEventListener("click", async () => {
    if (!path) return;
    if (printing) { await call("ide_hw_stop", { path }).catch(() => {}); await pollJobs(); setPrinting(false, "Stopped."); return; }
    if (!built) { $("wbPrintSt").textContent = "Build it first: there is no app to run yet."; return; }
    $("wbJobs").innerHTML = ""; shownJobs = 0;
    try {
      const name = await call("ide_hw_run", { path });
      setPrinting(true, `Running with ${name}. Print something from the app's window.`);
    } catch (err) {
      setPrinting(false, String(err));
    }
  });

  /* ---- the IDE's events ------------------------------------------------ */
  window.addEventListener("kr-ide-open", (e) => {
    if (path && printing) call("ide_hw_stop", { path }).catch(() => {});
    setPrinting(false, "When the app asks for a printer, it gets this one. Its labels appear here as they would come out: text, boxes, barcodes and QR codes at 203 dpi.");
    $("wbJobs").innerHTML = ""; shownJobs = 0;
    path = e.detail.path; built = false; asks = []; off = new Set();
    $("wbPerms").innerHTML = `<p class="ip-none">Build it to see what it asks for.</p>`;
    $("wbPermShot").classList.add("hidden");
    $("wbCards").innerHTML = "";
    $("wbCheck").textContent = "Run checks";
    pickDevice("window");
    tab("dev");
  });
  window.addEventListener("kr-ide-built", (e) => {
    if (e.detail.path !== path) return;
    built = true;
    loadAsks().then(() => { if (off.size) reactShot(); });
    if (device !== "window") shoot();
  });
})();
