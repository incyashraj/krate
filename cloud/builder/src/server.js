/* The build service: what happens when someone types a sentence on the
 * website and presses send.
 *
 * It runs `krate create` -- the same command the CLI and Studio run, not a
 * reimplementation -- so an app made in a browser and an app made on a
 * desktop cannot come out different. Everything else here is bookkeeping
 * around that one call: a queue, so one account cannot start ten builds;
 * stages parsed from the engine's own words; and a wall, because every
 * build is real money spent on real inference.
 *
 * Deliberately small and dependency-free. It holds an AI key and spends
 * money, and both of those argue for code that one person can read in one
 * sitting.
 */

import { createServer } from "node:http";
import { spawn } from "node:child_process";
import { randomBytes } from "node:crypto";
import {
  appendFile,
  mkdir,
  mkdtemp,
  readFile,
  readdir,
  rename,
  rm,
  stat,
  unlink,
  writeFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

const PORT = Number(process.env.PORT || 8787);
const KRATE = process.env.KRATE_BIN || "krate";
const AGENT = process.env.KRATE_AGENT || "claude";
const HUB = process.env.KRATE_HUB || "https://hub.krate.tech";

/* How long one build may take before it is called dead. Real builds are
 * minutes; a run past this is stuck, and a stuck run holds a queue slot
 * that a paying person is waiting on. */
const BUILD_TIMEOUT_MS = Number(process.env.KRATE_BUILD_TIMEOUT_MS || 15 * 60 * 1000);
const PLAN_TIMEOUT_MS = Number(process.env.KRATE_PLAN_TIMEOUT_MS || 90 * 1000);

/* One at a time per account. Not politeness -- the difference between a
 * bill and a bankruptcy, since each build is inference we pay for. */
const jobs = new Map();          // id -> job
const activeByAccount = new Map(); // account -> job id

/* ---- durable state -------------------------------------------------------
 * The machine stops when nobody is building and starts on the next request.
 * Jobs used to live only in this process, so every one of those restarts
 * silently forgot them: a person polling their build id got "no such build"
 * with no way to tell a typo from a lost result. Job records and finished
 * bundles now live on the volume, and a restart answers honestly instead.
 */
const STATE_DIR = process.env.KRATE_STATE_DIR || "/work/state";
const JOBS_DIR = join(STATE_DIR, "jobs");
let stateWritable = false;

/* A finished bundle is handed over and then expires. We are not the file's
 * host: the download exists so the person can save their app, not so a URL
 * can serve it forever. */
const RESULT_TTL_MS = Number(process.env.KRATE_RESULT_TTL_MS || 60 * 60 * 1000);

async function initState() {
  try {
    await mkdir(JOBS_DIR, { recursive: true });
    stateWritable = true;
  } catch (err) {
    // No volume (a dev checkout, a test without the env var pointing
    // anywhere writable): the service still works, jobs are just mortal.
    console.error(`state dir unavailable (${err.message}); jobs will not survive a restart`);
    return;
  }

  // Every record left by the previous life of this process. A job that was
  // mid-build when the machine stopped cannot be resumed -- the compiler
  // died with the process -- so it is reported as what it is, not left
  // "working" forever and not forgotten.
  let names = [];
  try { names = await readdir(JOBS_DIR); } catch (e) { return; }
  for (const name of names) {
    if (!name.endsWith(".json")) continue;
    try {
      const job = JSON.parse(await readFile(join(JOBS_DIR, name), "utf8"));
      job.proc = null;
      if (job.state === "working") {
        job.state = "failed";
        job.error = "The build machine restarted while this was being made. Start it again -- a failed build never costs a free one.";
        await persistJob(job);
      }
      jobs.set(job.id, job);
    } catch (e) { /* a torn write from a crash; skip it */ }
  }
}

/* What survives a restart: the record, never the process, and the bytes as
 * their own file so the JSON stays small enough to write atomically. */
async function persistJob(job) {
  if (!stateWritable) return;
  const { proc, result, ...record } = job;
  if (result) {
    record.result = { ...result, bytes: undefined, onDisk: true };
  }
  try {
    // Write-then-rename, so a crash mid-write leaves the old record whole
    // rather than a torn file the boot loader has to skip.
    const path = join(JOBS_DIR, `${job.id}.json`);
    await writeFile(`${path}.tmp`, JSON.stringify(record));
    await rename(`${path}.tmp`, path);
  } catch (e) { /* the next state change tries again */ }
}

async function persistResultBytes(job) {
  if (!stateWritable || !job.result || !job.result.bytes) return;
  try {
    await writeFile(join(JOBS_DIR, `${job.id}.krate`), job.result.bytes);
  } catch (e) { /* the in-memory copy still serves until restart */ }
}

async function resultBytes(job) {
  if (job.result && job.result.bytes) return job.result.bytes;
  if (job.result && job.result.onDisk) {
    try { return await readFile(join(JOBS_DIR, `${job.id}.krate`)); } catch (e) { return null; }
  }
  return null;
}

/* Results expire; expired jobs say so rather than vanish. */
async function expireOldResults() {
  const now = Date.now();
  for (const job of jobs.values()) {
    if (job.state !== "done" || !job.finished) continue;
    if (now - job.finished < RESULT_TTL_MS) continue;
    job.state = "expired";
    job.result = null;
    job.error = "This finished a while ago and the download has expired. Start the build again if you still need the file.";
    try { await unlink(join(JOBS_DIR, `${job.id}.krate`)); } catch (e) {}
    await persistJob(job);
    await audit({ action: "expire", account: job.account, job: job.id });
  }
}

/* ---- who is asking -------------------------------------------------------
 * Every operation on a job -- status, download, stop -- must come from the
 * account that started it. The hub is the authority; a short cache keeps a
 * two-second poll from becoming a hub request per poll. Cached by token, so
 * a refreshed sign-in (new token, same account) just takes one more lookup,
 * and an expired one stops working when the hub says so.
 */
const accountCache = new Map(); // token -> { account, until }
const ACCOUNT_CACHE_MS = 60 * 1000;

async function resolveAccount(token) {
  if (process.env.KRATE_BUILDER_DEV === "1") return "dev";
  if (!token) return null;
  const hit = accountCache.get(token);
  if (hit && hit.until > Date.now()) return hit.account;
  try {
    const res = await fetch(`${HUB}/me`, { headers: { authorization: `Bearer ${token}` } });
    if (!res.ok) { accountCache.delete(token); return null; }
    const me = await res.json();
    const account = (me.user && (me.user.login || me.user.email)) || null;
    if (account) accountCache.set(token, { account, until: Date.now() + ACCOUNT_CACHE_MS });
    return account;
  } catch (e) {
    // The hub being unreachable must not grant access; a stale cache entry
    // (checked above) is the only grace.
    return null;
  }
}

/* ---- audit ---------------------------------------------------------------
 * One line per thing that happened to a job: started, finished, failed,
 * stopped, downloaded, denied, expired. Not per poll -- a status check is
 * reading, not doing. JSON lines, append-only, on the volume.
 */
async function audit(entry) {
  if (!stateWritable) return;
  const line = JSON.stringify({ at: new Date().toISOString(), ...entry }) + "\n";
  try { await appendFile(join(STATE_DIR, "audit.log"), line); } catch (e) {}
}

/* The engine's own progress vocabulary, mapped to the four stages Studio
 * shows. These patterns are lifted from studio/ui/app.js so the web and
 * the desktop describe the same build in the same words -- if the engine's
 * wording changes, both must change together. */
const STAGE_RULES = [
  { stage: "read",  re: /^\s*\d*\.?\s*reading |authoring|starter/i },
  { stage: "write", re: /writing (the app's code|a file)|writing .*\.rs|setting up the build|declaring what the app needs/i },
  { stage: "test",  re: /checking it builds|running your app to test|opening your app to see|looking at how your app|==> building|Compiling|Generating bindings/i },
  { stage: "done",  re: /==> packing|==> verifying/i },
];
const STAGE_ORDER = ["read", "write", "test", "done"];

/* Whether this box can author at all.
 *
 * The service can be deployed with no AI key -- and is, deliberately, so the
 * machine and its DNS can go live and be checked before a single build is
 * paid for. What must not happen is accepting the request anyway: `krate
 * create` would start, spend a minute setting up, and die with the engine's
 * own error, which the person reads as "this product is broken" rather than
 * "this is switched off".
 *
 * So the refusal happens at the door, before a process is spawned, and it
 * says which of the two it is.
 *
 * A CLI provider needs no key here: it carries its own sign-in. Only the API
 * vendors spend a key that has to be present in this environment.
 */
const API_AGENTS = { anthropic: "ANTHROPIC_API_KEY", openai: "OPENAI_API_KEY" };

function authoringOff() {
  const needs = API_AGENTS[AGENT];
  if (!needs) return null;
  if (process.env[needs]) return null;
  return "Making apps in the browser is not switched on yet. Krate Studio is free, and it makes apps on your own machine.";
}

/* The caller's own API key, if they have set one.
 *
 * Asked with THEIR session, so the hub only ever decrypts a key for the
 * account that owns it and only while that account is building. Returns
 * null when they have none, which is the ordinary case for a first app.
 *
 * A failure here is a null, never an error: the funded path still works,
 * and a person whose key could not be read gets a build rather than a
 * broken page. What they must never get is a SILENT charge to us when they
 * believed they were paying -- so the caller checks which key it used and
 * records the spend against the right bucket.
 */
async function ownKey(token, vendor) {
  if (!token) return null;
  try {
    const res = await fetch(`${HUB}/keys/use`, {
      method: "POST",
      headers: { "content-type": "application/json", authorization: `Bearer ${token}` },
      body: JSON.stringify({ vendor }),
    });
    if (!res.ok) return null;
    const out = await res.json();
    return out.key || null;
  } catch (e) {
    return null;
  }
}

/* Tell the hub what a build cost, so the person can see it.
 *
 * Best effort and never in the way of a build: a ledger entry that does not
 * land is worth less than the app the person is waiting for.
 */
async function noteSpend(token, entry) {
  if (!token || !entry || !(entry.usd > 0)) return;
  try {
    await fetch(`${HUB}/spend`, {
      method: "POST",
      headers: { "content-type": "application/json", authorization: `Bearer ${token}` },
      body: JSON.stringify(entry),
    });
  } catch (e) {}
}

/* The engine prices every run from the API's own token counts and prints it
 * as `krate-spend: {...}`. This is the only honest source for what a build
 * cost -- anything else would be a guess presented as a number. */
function spendFromLines(lines) {
  for (let i = lines.length - 1; i >= 0; i--) {
    const m = String(lines[i]).match(/^krate-spend:\s*(\{.*\})\s*$/);
    if (!m) continue;
    try {
      const parsed = JSON.parse(m[1]);
      if (Number.isFinite(parsed.usd)) return parsed;
    } catch (e) {}
  }
  return null;
}

/* ---- the wall ------------------------------------------------------------
 * Asked of the hub, never of the browser. A counter the page owns is a
 * counter anyone can edit, and this one costs us money to be wrong about.
 */
async function allowedToBuild(token, device) {
  // A local escape hatch for developing the service itself, so testing the
  // build path does not need a real session. Opt-in through an env var
  // production never sets, and the ONLY way past the wall.
  if (process.env.KRATE_BUILDER_DEV === "1") {
    return { ok: true, account: "dev" };
  }
  if (!token) return { ok: false, message: "Sign in first -- it is how we know the first one is on us." };
  try {
    const res = await fetch(`${HUB}/me`, { headers: { authorization: `Bearer ${token}` } });
    if (!res.ok) return { ok: false, message: "That sign-in has expired. Sign in once more." };
    const me = await res.json();
    if (me.plan && me.plan.active) return { ok: true, account: me.user.login || me.user.email };

    // Both keys, so a new account on the same machine does not reset the
    // allowance. The hub answers with the higher of the two.
    const count = await fetch(`${HUB}/plan/get`, {
      method: "POST",
      headers: { "content-type": "application/json", authorization: `Bearer ${token}` },
      body: JSON.stringify({ device: device || "" }),
    });
    const made = count.ok ? (await count.json()).n || 0 : 0;

    // ONE app in the browser, then the desktop.
    //
    // Not a paywall -- Studio is free and unlimited, and the point of this
    // wall is to move somebody onto it rather than to charge them. Every
    // browser build costs real compute here, and the second one buys
    // nothing a download would not do better.
    //
    // The refusal names where their work went, because the session is
    // already on their account: signing into Studio brings the conversation
    // with it, editable. A wall that just says no would throw that away.
    // Somebody paying for their own inference is not using our funding, so
    // the one-app wall does not apply to them. That is the whole bargain of
    // bringing a key: Krate pays for the first app, and after it you either
    // move to Studio or bring your own key and carry on here.
    if (API_AGENTS[AGENT] && (await ownKey(token, AGENT))) {
      // `me.user.login || me.user.email`, the same expression the paid-plan
      // branch uses above. It said `account`, which is not a variable in
      // this scope -- a ReferenceError, thrown on the one line that lets a
      // bring-your-own-key person past the wall.
      //
      // The throw was caught by this function's own catch and reported as
      // "We could not check your plan just now": a 401 blaming the hub for
      // our typo. So the bargain the wall's message offers -- add your own
      // key and carry on here -- could not be taken by anyone.
      //
      // Never caught because the test suite runs KRATE_AGENT=claude, for
      // which API_AGENTS[AGENT] is undefined and this branch is unreachable.
      // Production runs `anthropic` (fly.toml), so the deployed
      // configuration was the one nothing exercised (K-765).
      return { ok: true, account: me.user.login || me.user.email, byok: true };
    }

    const BROWSER_FREE_BUILDS = 1;
    if (made >= BROWSER_FREE_BUILDS) {
      return {
        ok: false,
        wall: true,
        download: true,
        message:
          "You have made your app. Krate Studio is free and unlimited, and " +
          "this session is waiting in it -- sign in and carry on editing. " +
          "Or add your own API key in Settings and keep building here.",
      };
    }

    // The old three-a-month paywall stays switched off. The count is still
    // taken and recorded, so the day KRATE_CHARGING is set the numbers are
    // already right and any future wall works on real history rather than
    // starting everyone at zero.
    if (process.env.KRATE_CHARGING === "1" && made >= 3) {
      return {
        ok: false,
        message: "You have made your three free apps. Studio is unlimited, $12 a month.",
        wall: true,
      };
    }
    return { ok: true, account: me.user.login || me.user.email };
  } catch (err) {
    // A hub we cannot reach is our fault, not theirs -- but we still do
    // not spend money on an account we could not check.
    return { ok: false, message: "We could not check your plan just now. Try again in a moment." };
  }
}

/* ---- running one build --------------------------------------------------- */

/* A revision continues the case the app was made in: same funding, the
 * app's own source as the starting point, `krate revise` doing the edit --
 * the same command the desktop runs, so a change made in a tab and a change
 * made in Studio cannot come out different. `revise` names the finished job
 * it starts from: { parentId, source, change, caseId, name }. */
async function startBuild({ request, token, account, device, revise = null, shape = "", attachments = [] }) {
  // 128 random bits. The id appears in URLs and is all a page holds, so it
  // must not be guessable -- the truncated UUID this used to be was the only
  // thing between anyone on the internet and another person's app file.
  // Ownership is checked on every operation now, but the id stays opaque:
  // two locks, and this one also keeps 404 and 403 indistinguishable.
  const id = randomBytes(16).toString("hex");
  const dir = await mkdtemp(join(tmpdir(), "krate-build-"));
  const output = join(dir, "app.krate");
  const shotPath = join(dir, "frame.png");

  const job = {
    id, account, device, request: revise ? revise.change : request, dir, output, shotPath,
    parent: revise ? revise.parentId : null,
    state: "working",
    stage: "read",
    line: "reading what Krate can do",
    shot: null,
    result: null,
    error: null,
    started: Date.now(),
    finished: null,
    proc: null,
  };
  jobs.set(id, job);
  activeByAccount.set(account, id);
  // The build lives inside a funded case on the hub's ledger. Opening it
  // consumes nothing; the outcome recorded when this build ends is what
  // decides whether it cost an allowance (only "made" does).
  //
  // A change opens its OWN case, flagged as an edit. It used to inherit the
  // parent's caseId, which meant a change never reached `/case/open` at all
  // -- so the hub's edit allowance, which is decided there, could never
  // fire, and the one free change was in truth unlimited. Inheriting also
  // made every retry of a change look like another attempt at the original
  // app, which is not what the ledger is counting.
  // A change arrives with its case already opened by `allowedToRevise`,
  // which had to ask the hub anyway to learn whether the change was
  // allowed at all. Opening a second one here would count it twice.
  job.caseId = revise ? revise.caseId : await caseOpen(token, device, request);
  await persistJob(job);
  await audit({ action: revise ? "revise" : "start", account, job: id, parent: job.parent });

  // The engine prints its progress as it works; that is what drives the
  // stages. The app's picture comes after, from running the finished file
  // (`krate create` has no --shoot; `krate run` does), which is also the
  // more honest picture: it is the app that was actually made.
  const transcript = join(dir, "transcript.json");
  // Their key if they have one, ours otherwise. Held in this process for
  // the life of one build and passed to the engine through the environment,
  // never written to the job record or to disk.
  const theirKey = API_AGENTS[AGENT] ? await ownKey(token, AGENT) : null;
  job.paidBy = theirKey ? "own" : "krate";
  // What the person attached, written into this build's own directory so
  // the engine can read it. `--attach` is repeatable and takes a path, so
  // the bytes a browser sent become real files exactly here. A failure to
  // write one must not fail the build: an app made from the words alone is
  // better than no app.
  let attachArgs = [];
  try {
    const paths = await writeAttachments(attachments, dir);
    attachArgs = paths.flatMap((path) => ["--attach", path]);
  } catch (e) {
    console.warn(`[build] could not write attachments: ${e.message}`);
  }
  const args = revise
    ? ["revise", revise.source, revise.change, "--agent", AGENT, "--output", output, ...attachArgs]
    : ["create", request, "--output", output, "--agent", AGENT, "--transcript", transcript, ...attachArgs];
  const runEnv = { ...process.env };
  if (theirKey) runEnv[API_AGENTS[AGENT]] = theirKey;
  // The shape the plan picked: the engine seeds that working example as
  // src/lib.rs and the model transforms it rather than writing a file from
  // nothing. This is the single biggest lever on how long a build takes,
  // and the browser was not using it.
  if (shape && /^[a-z0-9-]{1,40}$/i.test(shape)) runEnv.KRATE_STARTER_SHAPE = shape;
  const proc = spawn(KRATE, args, { cwd: dir, env: runEnv });
  job.proc = proc;

  let tail = "";
  const onChunk = (buf) => {
    tail = (tail + buf.toString()).slice(-4000);
    for (const raw of buf.toString().split("\n")) {
      const line = raw.trim();
      if (!line) continue;
      // Stages only move forward. The engine alternates between writing
      // and checking, and a bar that goes backwards reads as a fault.
      for (const rule of STAGE_RULES) {
        if (rule.re.test(line) && STAGE_ORDER.indexOf(rule.stage) > STAGE_ORDER.indexOf(job.stage)) {
          job.stage = rule.stage;
        }
      }
      // What the run has cost so far, priced by the engine from the API's
      // own token counts. Kept as it goes by: the last one wins, and a
      // build that ends badly still spent what it spent.
      const spent = spendFromLines([line]);
      if (spent) job.spend = spent;
      // The engine's own sentence, when it is one a person can read.
      if (/^[a-z]/.test(line) && line.length < 90 && !line.startsWith("==>")) {
        job.line = line;
      }
    }
  };
  proc.stdout.on("data", onChunk);
  proc.stderr.on("data", onChunk);

  const killer = setTimeout(() => {
    job.error = "This one took too long and was stopped.";
    try { proc.kill("SIGKILL"); } catch (e) {}
  }, BUILD_TIMEOUT_MS);

  proc.on("close", async (code) => {
    clearTimeout(killer);
    activeByAccount.delete(account);

    if (job.state === "stopped") {
      await persistJob(job);
      await audit({ action: "stopped", account, job: job.id });
      await caseAttempt(token, device, job.caseId, "stopped");
      return cleanup(job);
    }
    // Exit 6 is the engine's own request verdict: the app built, runs, and
    // is not what was asked for (`krate create` says so and keeps the
    // file). A person must see that verdict beside the app, not "it
    // failed" and not "here is your app". It spends nothing: the funded
    // case continues until an app that serves the request exists, and the
    // next change starts from this one.
    const offRequest = code === 6 && !job.error && (await stat(output).catch(() => null));
    if ((code !== 0 || job.error) && !offRequest) {
      const timedOut = Boolean(job.error);
      job.state = "failed";
      job.error = job.error || plainFailure(tail);
      job.finished = Date.now();
      await persistJob(job);
      await audit({ action: "failed", account, job: job.id });
      // A failure on our side costs the person nothing: the case records it
      // and stays open for a free retry. The timeout is this machine being
      // slow (infra); anything else out of `krate create` is the engine or
      // the provider, and "krate-failed" is the honest default when the
      // exit code cannot tell them apart.
      await caseAttempt(
        token, device, job.caseId,
        timedOut ? "infra-failed" : "krate-failed",
        tail.split("\n").filter(Boolean).pop() || "",
      );
      await noteSpend(token, job.spend && {
        ...job.spend,
        app: "(build failed)",
        paid_by: job.paidBy || "krate",
      });
      return cleanup(job);
    }

    try {
      const bytes = await readFile(output);
      const info = await stat(output);
      // The picture of the app that was just made, painted by the same
      // renderer the desktop uses, so the preview cannot flatter it.
      job.stage = "done";
      job.line = "taking its picture";
      job.shot = await takeShot(output, shotPath);
      // The engine's own record of the build: what the app asks the
      // person for (the done card shows it, as Studio does on a desktop)
      // and, when the app is not what was asked, the reason.
      let verdict = null;
      let asks = [];
      try {
        const written = JSON.parse(await readFile(transcript, "utf8"));
        asks = Array.isArray(written.requested_permissions)
          ? written.requested_permissions.map((p) => (typeof p === "string" ? p : p.cap || p.capability || "")).filter(Boolean)
          : [];
        if (offRequest) {
          verdict = String(written.verdict || "").replace(/^built a working, permission-gated \.krate, but it does not serve the request: ?/, "").trim();
        }
      } catch (e) {}
      if (offRequest) job.line = "built, but it is not what you asked for";
      job.result = {
        id,
        name: revise ? revise.name : prettyName(request),
        size: prettySize(info.size),
        asks,
        shot: job.shot,
        // "off-request": the engine's verdict that the app does not serve
        // the request, with its reason; null when the app was accepted.
        verdict: offRequest ? "off-request" : null,
        verdict_detail: verdict || null,
        // Held in memory and handed over on download. The file is the
        // product; we are not its host.
        bytes,
      };
      job.state = "done";
      job.finished = Date.now();
      // The bundle goes to the volume before the record says "done", so a
      // restart between the two cannot leave a record that promises a file
      // the disk does not have.
      await persistResultBytes(job);
      await persistJob(job);
      await audit({ action: "done", account, job: job.id });
      // Only a build that produced a file the engine accepted counts
      // against the allowance: "made" is the one outcome that consumes the
      // case's funding. An app that is not what was asked is recorded as
      // "off-request" and the case stays open for the change that fixes it.
      await caseAttempt(token, device, job.caseId, offRequest ? "off-request" : "made", verdict || "");
      // What it cost, against the right money: their key or ours.
      await noteSpend(token, job.spend && {
        ...job.spend,
        app: job.result ? job.result.name : "",
        paid_by: job.paidBy || "krate",
      });
    } catch (err) {
      job.state = "failed";
      job.error = "The app was made but could not be read back.";
      job.finished = Date.now();
      await persistJob(job);
      await audit({ action: "failed", account, job: job.id });
    }
    cleanup(job, { keepFile: true });
  });

  return job;
}

/* `krate plan <request>`: the engine's own pre-build answer, as JSON text.
 * Bounded, because a model that never answers must not hold the request
 * open; and one answer at a time is plenty. */
/// The most one attachment may be, and the most one request may carry.
///
/// A browser sends bytes as base64, which is about a third larger than
/// the file, so the wire budget is set from the decoded size and the
/// body limit is raised to match.
const MAX_ATTACH_BYTES = 10 * 1024 * 1024;
const MAX_ATTACHMENTS = 6;

/* Write what the person attached into a directory the engine can read.
 *
 * A browser has no paths to send, so it sends names and bytes. They
 * become real files here, and `--attach <file>` points the engine at
 * them. Returns the paths written, in the order given.
 *
 * The name is NOT trusted: it arrives from a browser and is used to
 * build a path. Only the basename is kept, anything outside a safe set
 * of characters is replaced, and an empty result gets a generated name.
 * A `../..` in an attachment name must not be able to write outside the
 * directory this creates.
 */
async function writeAttachments(list, dir) {
  const items = Array.isArray(list) ? list.slice(0, MAX_ATTACHMENTS) : [];
  if (!items.length) return [];
  const into = join(dir, "attachments");
  await mkdir(into, { recursive: true });
  const written = [];
  for (const [index, item] of items.entries()) {
    if (!item || typeof item.bytes !== "string") continue;
    const bytes = Buffer.from(item.bytes, "base64");
    if (!bytes.length || bytes.length > MAX_ATTACH_BYTES) continue;
    const raw = String(item.name || "").split(/[\\/]/).pop() || "";
    const safe = raw.replace(/[^A-Za-z0-9._-]+/g, "-").replace(/^[.-]+/, "");
    const name = safe || `attachment-${index + 1}`;
    const path = join(into, name);
    await writeFile(path, bytes);
    written.push(path);
  }
  return written;
}

async function planRequest(request, theirKey = null, attachments = []) {
  // The plan reads what was attached too: `krate plan --attach` exists
  // precisely so the questions can be about the file rather than only the
  // sentence. Written to a temp dir that is removed once the plan answers.
  const holding = await mkdtemp(join(tmpdir(), "krate-plan-"));
  let files = [];
  try {
    files = await writeAttachments(attachments, holding);
  } catch (e) { /* a plan without the file is better than no plan */ }
  const attachArgs = files.flatMap((f) => ["--attach", f]);
  return new Promise((resolve) => {
    const finish = (value) => {
      rm(holding, { recursive: true, force: true }).catch(() => {});
      resolve(value);
    };
    let out = "";
    let err = "";
    const env = { ...process.env };
    if (theirKey) env[API_AGENTS[AGENT]] = theirKey;
    const proc = spawn(KRATE, ["plan", request, "--agent", AGENT, ...attachArgs], { env });
    const killer = setTimeout(() => { try { proc.kill("SIGKILL"); } catch (e) {} }, PLAN_TIMEOUT_MS);
    proc.stdout.on("data", (b) => { out += b.toString(); });
    proc.stderr.on("data", (b) => { err = (err + b.toString()).slice(-2000); });
    proc.on("error", (e) => { clearTimeout(killer); finish({ ok: false, message: `could not run the engine: ${e.message}` }); });
    proc.on("close", (code) => {
      clearTimeout(killer);
      const text = out.trim();
      if (code !== 0 || !text) {
        return finish({ ok: false, message: plainFailure(err || "the plan step failed") });
      }
      try { JSON.parse(text); } catch (e) {
        return finish({ ok: false, message: "the plan step answered with something that is not a plan" });
      }
      finish({ ok: true, text });
    });
  });
}

/* Run the finished app once, headless, and keep the frame. A build with no
 * picture is still a build -- the app is the product -- so a failure here
 * is swallowed rather than allowed to fail the job. */
async function takeShot(bundle, shotPath) {
  return new Promise((resolve) => {
    // --auto-grant, NOT --consent. `--consent` opens a native permission
    // window on macOS and falls back to a TERMINAL PROMPT everywhere else --
    // so on the Linux build box any app that asked for a capability (a
    // checklist asking to save its items is enough) stopped dead waiting for
    // an answer from a terminal that does not exist, and exited 5. The
    // failure was invisible: this function swallows errors by design, so the
    // app still shipped, just with no picture, and nothing said why.
    //
    // Granting everything is right here and only here: this is Krate running
    // an app it just built itself, for one headless second, to photograph it.
    // Nobody is being asked to trust anything -- the consent that matters
    // happens on the person's own machine when they open the file.
    const proc = spawn(KRATE, ["run", bundle, "--shoot", shotPath, "--auto-grant"], {
      env: { ...process.env, KRATE_SHOOT_AFTER_MS: "1200" },
    });
    // Why a picture failed, kept for the log.
    //
    // This function resolves null for every failure -- a crash, a timeout,
    // a missing audio device, an app that drew nothing -- and said nothing
    // about which. So "no preview for this one" was not diagnosable from
    // the outside at all: the only way to learn why was to reproduce the
    // build by hand. The engine's own output is the answer, and it was
    // being thrown away.
    let tail = "";
    const keep = (b) => { tail = (tail + b.toString()).slice(-800); };
    proc.stdout?.on("data", keep);
    proc.stderr?.on("data", keep);
    let killed = false;
    const give = setTimeout(() => {
      killed = true;
      try { proc.kill("SIGKILL"); } catch (e) {}
    }, 45000);
    const giveUp = (why) => {
      console.warn(`[shot] no picture: ${why}${tail ? `\n[shot] engine said: ${tail.trim()}` : ""}`);
      resolve(null);
    };
    proc.on("close", async (code) => {
      clearTimeout(give);
      try {
        const png = await readFile(shotPath);
        resolve(`data:image/png;base64,${png.toString("base64")}`);
      } catch (e) {
        giveUp(killed ? "the app was still running after 45s" : `exit ${code}, and no file was written`);
      }
    });
    proc.on("error", (err) => {
      clearTimeout(give);
      giveUp(`could not run the engine: ${err.message}`);
    });
  });
}

/* ---- the funded case (IC-001) --------------------------------------------
 * Every build lives inside a case on the hub's ledger. Opening one consumes
 * nothing; only an attempt that produced a file does. So a build that dies
 * on our side -- the provider, this machine, the engine -- is recorded as
 * exactly that and costs the person nothing, where the old counter bumped
 * the same number for every outcome it managed to reach.
 *
 * Every call is fire-and-forget past the open: a hub outage must not turn
 * into a failed build, and the ledger self-heals -- planCount mirrors and
 * migration mints -- when the hub is back.
 */
async function caseOpen(token, device, request, edit = false) {
  try {
    const res = await fetch(`${HUB}/case/open`, {
      method: "POST",
      headers: { "content-type": "application/json", authorization: `Bearer ${token}` },
      body: JSON.stringify({ device: device || "", request, edit }),
    });
    if (!res.ok) return null;
    const body = await res.json();
    return body.id || null;
  } catch (e) {
    return null;
  }
}

/* May this person change their app?
 *
 * Asked BEFORE a change starts, because `caseOpen` cannot answer it: a
 * refused case returns null there, and a null caseId means "no ledger",
 * which starts the build anyway. So the wall is read here, where a 402
 * can still stop the work and be shown.
 *
 * Shaped like `allowedToBuild`'s answer so the route handles both the same
 * way. The hub owns the decision; a browser-side count is a count anyone
 * can edit, and this one costs us money to be wrong about.
 */
async function allowedToRevise(token, device, change) {
  if (process.env.KRATE_BUILDER_DEV === "1") return { ok: true, caseId: null };
  let res;
  try {
    res = await fetch(`${HUB}/case/open`, {
      method: "POST",
      headers: { "content-type": "application/json", authorization: `Bearer ${token}` },
      body: JSON.stringify({ device: device || "", request: change || "", edit: true }),
    });
  } catch (e) {
    // The hub is unreachable. A change is cheap next to telling somebody
    // their work is blocked because our ledger had a bad minute, so this
    // fails open -- exactly as an unreachable `caseOpen` always has.
    return { ok: true, caseId: null };
  }
  if (res.ok) {
    const body = await res.json().catch(() => ({}));
    return { ok: true, caseId: (body && body.id) || null };
  }
  // 402 is the wall, 409 is "there is no app to change yet". Both are
  // answers with a sentence written for the person, and both must stop the
  // build -- which is why this is asked here and not inside `caseOpen`,
  // where a refusal is indistinguishable from "no ledger, carry on".
  if (res.status === 402 || res.status === 409) {
    const body = await res.json().catch(() => ({}));
    return {
      ok: false,
      wall: true,
      download: res.status === 402,
      message: body.message || "That was your free change.",
    };
  }
  return { ok: true, caseId: null };
}

async function caseAttempt(token, device, caseId, outcome, note) {
  if (!caseId) return;
  await fetch(`${HUB}/case/attempt`, {
    method: "POST",
    headers: { "content-type": "application/json", authorization: `Bearer ${token}` },
    body: JSON.stringify({ device: device || "", id: caseId, outcome, note }),
  }).catch(() => {});
}

async function cleanup(job, opts = {}) {
  // The bytes are already in memory by now; the directory is scratch.
  try { await rm(job.dir, { recursive: true, force: true }); } catch (e) {}
  if (!opts.keepFile) job.result = null;
}

/* The engine's failures are for us; the person gets a sentence. The tail
 * is kept on the job for a report, never shown raw on a page. */
function plainFailure(tail) {
  // Everything on OUR side of the line reads the same to a person: it did
  // not work, it was not their fault, and trying again is reasonable. An
  // expired OAuth token is our problem, and it must never appear on a page
  // where a stranger is deciding whether this product is real.
  if (/oauth|session expired|not signed in|no api key|unauthor|rate limit|quota|could not write the app/i.test(tail)) {
    return "Our AI could not be reached just now. This one is on us -- try again in a minute.";
  }
  // The spend ceiling stopped a build that was not converging. Not a fault
  // and not a refusal: the request was simply bigger than one build can
  // carry, and the useful next move is a smaller one. Said without mentioning
  // money, because the ceiling is ours and a person asking for an app should
  // not have to think about our bill.
  if (/more work than we allow|ceiling/i.test(tail)) {
    return "That one grew bigger than a single build can carry. Try asking for the smaller version first -- you can add to it in Studio.";
  }
  // The permission wall refusing a request is a different thing entirely:
  // the person asked for something Krate will not do, and saying so is the
  // wall working rather than a failure.
  // "cannot build that" is the engine's actual wording; matching only
  // "cannot do" meant a real refusal fell through to the generic sentence,
  // so the one failure that HAS an actionable next step was the one that
  // did not offer it.
  if (/refus|cannot build|cannot do|will not/i.test(tail)) {
    return "That asks for something Krate cannot do yet. Try describing it another way.";
  }
  if (/timed out|timeout/i.test(tail)) {
    return "That took longer than we allow. Try a smaller first version.";
  }
  return "That one didn't come together. Your words are still here.";
}

function prettyName(request) {
  const words = String(request).trim().split(/\s+/).slice(0, 4).join(" ");
  const clean = words.replace(/^(a|an|the|make|build|create)\s+/i, "");
  return clean ? clean[0].toUpperCase() + clean.slice(1) : "Your app";
}

function prettySize(bytes) {
  return bytes < 1024 * 1024
    ? `${Math.round(bytes / 1024)} KB`
    : `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

/* ---- the doors ----------------------------------------------------------- */

const server = createServer(async (req, res) => {
  const url = new URL(req.url, `http://${req.headers.host}`);
  const token = (req.headers.authorization || "").replace(/^Bearer\s+/i, "").trim();

  // The page and the service are on different origins by design: the
  // service is a box we run, the page is on the CDN.
  res.setHeader("access-control-allow-origin", process.env.KRATE_ORIGIN || "*");
  res.setHeader("access-control-allow-headers", "authorization, content-type");
  res.setHeader("access-control-allow-methods", "GET, POST, OPTIONS");
  if (req.method === "OPTIONS") return send(res, 204, "");

  try {
    if (req.method === "GET" && url.pathname === "/health") {
      // `authoring` is the answer to "is this box switched on", which is
      // otherwise only discoverable by starting a build. It reports whether
      // a key is present, never anything about the key itself.
      return json(res, 200, {
        ok: true,
        building: activeByAccount.size,
        authoring: authoringOff() ? "off" : "on",
        agent: AGENT,
      });
    }

    if (req.method === "POST" && url.pathname === "/build") {
      const body = await readBody(req);
      const request = String(body.request || "").trim();
      const device = String(body.device || "").trim();
      if (!request) return send(res, 400, "Say what to make.");
      if (request.length > 2000) return send(res, 400, "That is longer than we can work from.");

      // Before the wall, and before any process: a box with no key of OUR
      // own cannot make anything, and saying so costs nothing. Sent as the
      // same shape the wall uses, so the page already knows how to offer
      // the download instead of showing a bare failure. Somebody who
      // brought their own key is not asking us to spend anything, so this
      // is not their wall -- `allowedToBuild` checks that for real and
      // lets them straight through.
      const off = authoringOff();
      if (off && !(API_AGENTS[AGENT] && (await ownKey(token, AGENT)))) {
        return json(res, 503, { wall: true, download: true, message: off });
      }

      const shape = String(body.shape || "").trim();
      const allowed = await allowedToBuild(token, device);
      if (!allowed.ok) {
        // JSON for a wall, plain text for everything else.
        //
        // The browser needs the `download` flag, not just a sentence: it
        // decides whether the card offers "Download Studio" or a plain
        // failure, and a message alone cannot carry that. Text stays for
        // real errors, which have nothing structured to say.
        if (allowed.wall) {
          return json(res, 402, {
            wall: true,
            download: Boolean(allowed.download),
            message: allowed.message,
          });
        }
        return send(res, 401, allowed.message);
      }

      if (activeByAccount.has(allowed.account)) {
        return send(res, 429, "One app is already being made. It will be a few minutes.");
      }

      const job = await startBuild({ request, token, account: allowed.account, device, shape, attachments: body.attachments });
      return json(res, 200, { id: job.id });
    }

    // The conversation before the build: the engine's `plan` step, which
    // answers with up to three questions or a one-paragraph plan and never
    // builds anything. It costs the model one short answer and consumes no
    // allowance: asking is not making.
    if (req.method === "POST" && url.pathname === "/plan") {
      const body = await readBody(req);
      const request = String(body.request || "").trim();
      if (!request) return send(res, 400, "Say what to make.");
      if (request.length > 2000) return send(res, 400, "That is longer than we can work from.");
      const account = await resolveAccount(token);
      if (!account) return send(res, 401, "Sign in first.");
      const theirs = API_AGENTS[AGENT] ? await ownKey(token, AGENT) : null;
      const off = authoringOff();
      if (off && !theirs) return json(res, 503, { wall: true, download: true, message: off });
      const answer = await planRequest(request, theirs, body.attachments);
      if (!answer.ok) return send(res, 502, answer.message);
      res.statusCode = 200;
      res.setHeader("content-type", "application/json");
      return res.end(answer.text);
    }

    if (req.method === "POST" && url.pathname.endsWith("/revise") && url.pathname.startsWith("/build/")) {
      const id = url.pathname.split("/")[2];
      const body = await readBody(req);
      const change = String(body.change || "").trim();
      const device = String(body.device || "").trim();
      if (!change) return send(res, 400, "Say what to change.");
      if (change.length > 2000) return send(res, 400, "That is longer than we can work from.");
      const account = await resolveAccount(token);
      if (!account) return send(res, 401, "Sign in first.");
      const off = authoringOff();
      if (off && !(API_AGENTS[AGENT] && (await ownKey(token, AGENT)))) {
        return json(res, 503, { wall: true, download: true, message: off });
      }
      const job = jobs.get(id);
      if (!job || job.account !== account) {
        if (job) await audit({ action: "denied", account, job: id });
        return send(res, 404, "no such build");
      }
      if (job.state === "expired") return send(res, 404, job.error);
      if (job.state !== "done" || !job.result) return send(res, 409, "Make the app first; a change starts from a finished one.");
      const bytes = await resultBytes(job);
      if (!bytes) return send(res, 404, "not ready");
      if (activeByAccount.has(account)) {
        return send(res, 429, "One app is already being made. It will be a few minutes.");
      }
      // One free change, like one free app. Asked before any work starts,
      // and before the source is copied, so a refusal costs nothing.
      //
      // This route had no allowance check at all: a change inherited the
      // parent's case, so `/case/open` -- the only place the hub decides an
      // edit -- was never reached, and the free change was unlimited in
      // practice however the hub was configured.
      const mayRevise = await allowedToRevise(token, device, change);
      if (!mayRevise.ok) {
        return json(res, 402, {
          wall: true,
          download: Boolean(mayRevise.download),
          message: mayRevise.message,
        });
      }
      // The source the change starts from is the file that was made -- the
      // one the person could have downloaded -- copied into the new job's
      // own directory so nothing edits a finished result in place.
      const sourceDir = await mkdtemp(join(tmpdir(), "krate-revise-"));
      const source = join(sourceDir, "app.krate");
      await writeFile(source, bytes);
      const next = await startBuild({
        request: job.request, token, account, device,
        attachments: body.attachments,
        revise: { parentId: id, source, change, caseId: mayRevise.caseId, name: job.result.name },
      });
      return json(res, 200, { id: next.id, parent: id });
    }

    if (req.method === "GET" && url.pathname.startsWith("/build/")) {
      const [, , id, action] = url.pathname.split("/");
      await expireOldResults();

      // Every operation on a job belongs to the account that started it.
      // These routes used to take no token at all: the truncated id was the
      // only secret, and anyone holding it could read the status, download
      // the finished app, or kill the build. Now the token is resolved to an
      // account and compared -- and a job that is not yours looks exactly
      // like a job that does not exist, so a probe learns nothing.
      const account = await resolveAccount(token);
      if (!account) return send(res, 401, "Sign in first.");
      const job = jobs.get(id);
      if (!job || job.account !== account) {
        if (job) await audit({ action: "denied", account, job: id });
        return send(res, 404, "no such build");
      }

      if (action === "file") {
        if (job.state === "expired") return send(res, 404, job.error);
        if (!job.result) return send(res, 404, "not ready");
        const bytes = await resultBytes(job);
        if (!bytes) return send(res, 404, "not ready");
        res.setHeader("content-type", "application/octet-stream");
        res.setHeader(
          "content-disposition",
          `attachment; filename="${job.result.name.replace(/[^a-z0-9]+/gi, "-").toLowerCase()}.krate"`,
        );
        await audit({ action: "download", account, job: id });
        return send(res, 200, bytes);
      }

      return json(res, 200, {
        state: job.state,
        stage: job.stage,
        line: job.line,
        shot: job.shot,
        parent: job.parent || null,
        error: job.error,
        result: job.result && {
          id: job.result.id,
          name: job.result.name,
          size: job.result.size,
          asks: job.result.asks,
          verdict: job.result.verdict || null,
          verdict_detail: job.result.verdict_detail || null,
          // Where the file is. The page fetches it with the same sign-in
          // (the endpoint reads the bearer token, so a bare link cannot),
          // and hands the bytes to the browser as a download. The two
          // surfaces used to read this field and it was never here, so
          // neither could ever download an app.
          download: `/build/${job.id}/file`,
          shot: job.result.shot,
          download: `/build/${job.id}/file`,
        },
      });
    }

    if (req.method === "POST" && url.pathname.endsWith("/stop")) {
      const id = url.pathname.split("/")[2];

      // Owned, like every other operation: stopping somebody else's build
      // is denying them the thing they are paying attention to.
      const account = await resolveAccount(token);
      if (!account) return send(res, 401, "Sign in first.");
      const job = jobs.get(id);
      if (!job || job.account !== account) {
        if (job) await audit({ action: "denied", account, job: id });
        return send(res, 404, "no such build");
      }

      // Idempotent. A double-tap on the stop button, or a stop after the
      // build already finished, changes nothing and says what is true.
      if (job.state === "working" && job.proc) {
        job.state = "stopped";
        try { job.proc.kill("SIGTERM"); } catch (e) {}
        activeByAccount.delete(job.account);
        await persistJob(job);
        await audit({ action: "stop", account, job: id });
      }
      return json(res, 200, { ok: true, state: job.state });
    }

    return send(res, 404, "not found");
  } catch (err) {
    return send(res, 500, "Something broke on our side.");
  }
});

function readBody(req) {
  return new Promise((resolve, reject) => {
    let data = "";
    req.on("data", (c) => {
      data += c;
      // Attachments ride in the body as base64, which is about a third
      // larger than the file. Six files at 10 MB each is the ceiling the
      // browser enforces, so this is that plus room for the request.
      if (data.length > 90_000_000) reject(new Error("too big"));
    });
    req.on("end", () => {
      try { resolve(data ? JSON.parse(data) : {}); } catch (e) { resolve({}); }
    });
    req.on("error", reject);
  });
}

function json(res, status, body) {
  res.setHeader("content-type", "application/json");
  send(res, status, JSON.stringify(body));
}

function send(res, status, body) {
  res.statusCode = status;
  res.end(body);
}

// State first, then the port: a request that arrives before the old records
// are loaded would answer "no such build" about a job the disk knows.
await initState();
server.listen(PORT, () => {
  console.log(`krate builder on :${PORT} (engine: ${KRATE}, agent: ${AGENT})`);
});

/* Shut down when told to.
 *
 * This process is PID 1 in its container, and PID 1 does NOT get the default
 * signal handlers -- an unhandled SIGTERM is simply ignored, so the host
 * waits out its whole grace period and then kills the machine. Every deploy
 * would stall, and any build running at the time would die at the hard kill
 * instead of the polite one.
 *
 * Handling it explicitly turns that into a real shutdown: stop taking new
 * work, end the builds in flight (their compilers are children and would
 * otherwise be orphaned), and go.
 */
let leaving = false;
for (const signal of ["SIGTERM", "SIGINT"]) {
  process.on(signal, () => {
    if (leaving) return;
    leaving = true;
    server.close();
    for (const job of jobs.values()) {
      if (job.proc && job.state === "working") {
        job.state = "stopped";
        try { job.proc.kill("SIGTERM"); } catch (e) {}
      }
    }
    // A moment for the children to go, then leave regardless. Waiting on
    // them forever would recreate the hang this handler exists to prevent.
    setTimeout(() => process.exit(0), 2000).unref();
  });
}
