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
import { randomUUID } from "node:crypto";
import { mkdtemp, readFile, rm, stat } from "node:fs/promises";
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

/* One at a time per account. Not politeness -- the difference between a
 * bill and a bankruptcy, since each build is inference we pay for. */
const jobs = new Map();          // id -> job
const activeByAccount = new Map(); // account -> job id

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
  if (!token) return { ok: false, message: "Sign in first -- it is how we know which three are free." };
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
    const BROWSER_FREE_BUILDS = 1;
    if (made >= BROWSER_FREE_BUILDS) {
      return {
        ok: false,
        wall: true,
        download: true,
        message:
          "You have made your app. Krate Studio is free and unlimited, and " +
          "this session is waiting in it -- sign in and carry on editing.",
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

async function startBuild({ request, token, account, device }) {
  const id = randomUUID().slice(0, 12);
  const dir = await mkdtemp(join(tmpdir(), "krate-build-"));
  const output = join(dir, "app.krate");
  const shotPath = join(dir, "frame.png");

  const job = {
    id, account, device, request, dir, output, shotPath,
    state: "working",
    stage: "read",
    line: "reading what Krate can do",
    shot: null,
    result: null,
    error: null,
    started: Date.now(),
    proc: null,
  };
  jobs.set(id, job);
  activeByAccount.set(account, id);

  // The engine prints its progress as it works; that is what drives the
  // stages. The app's picture comes after, from running the finished file
  // (`krate create` has no --shoot; `krate run` does), which is also the
  // more honest picture: it is the app that was actually made.
  const args = ["create", request, "--output", output, "--agent", AGENT];
  const proc = spawn(KRATE, args, { cwd: dir, env: { ...process.env } });
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

    if (job.state === "stopped") return cleanup(job);
    if (code !== 0 || job.error) {
      job.state = "failed";
      job.error = job.error || plainFailure(tail);
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
      job.result = {
        id,
        name: prettyName(request),
        size: prettySize(info.size),
        asks: [],
        shot: job.shot,
        // Held in memory and handed over on download. The file is the
        // product; we are not its host.
        bytes,
      };
      job.state = "done";
      // Only a build that produced a file counts against the free three.
      // A failure the person did not cause must never cost them one.
      countTheMake(token, job.device).catch(() => {});
    } catch (err) {
      job.state = "failed";
      job.error = "The app was made but could not be read back.";
    }
    cleanup(job, { keepFile: true });
  });

  return job;
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
    const give = setTimeout(() => { try { proc.kill("SIGKILL"); } catch (e) {} }, 45000);
    proc.on("close", async () => {
      clearTimeout(give);
      try {
        const png = await readFile(shotPath);
        resolve(`data:image/png;base64,${png.toString("base64")}`);
      } catch (e) {
        resolve(null);
      }
    });
    proc.on("error", () => { clearTimeout(give); resolve(null); });
  });
}

async function countTheMake(token, device) {
  await fetch(`${HUB}/plan/count`, {
    method: "POST",
    headers: { "content-type": "application/json", authorization: `Bearer ${token}` },
    body: JSON.stringify({ device: device || "" }),
  });
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
  // The permission wall refusing a request is a different thing entirely:
  // the person asked for something Krate will not do, and saying so is the
  // wall working rather than a failure.
  if (/refus|cannot do|will not/i.test(tail)) {
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

      // Before the wall, and before any process: a box with no key cannot
      // make anything, and saying so costs nothing. Sent as the same shape
      // the wall uses, so the page already knows how to offer the download
      // instead of showing a bare failure.
      const off = authoringOff();
      if (off) {
        return json(res, 503, { wall: true, download: true, message: off });
      }

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

      const job = await startBuild({ request, token, account: allowed.account, device });
      return json(res, 200, { id: job.id });
    }

    if (req.method === "GET" && url.pathname.startsWith("/build/")) {
      const [, , id, action] = url.pathname.split("/");
      const job = jobs.get(id);
      if (!job) return send(res, 404, "no such build");

      if (action === "file") {
        if (!job.result) return send(res, 404, "not ready");
        res.setHeader("content-type", "application/octet-stream");
        res.setHeader(
          "content-disposition",
          `attachment; filename="${job.result.name.replace(/[^a-z0-9]+/gi, "-").toLowerCase()}.krate"`,
        );
        return send(res, 200, job.result.bytes);
      }

      return json(res, 200, {
        state: job.state,
        stage: job.stage,
        line: job.line,
        shot: job.shot,
        error: job.error,
        result: job.result && {
          id: job.result.id,
          name: job.result.name,
          size: job.result.size,
          asks: job.result.asks,
          shot: job.result.shot,
          download: `/build/${job.id}/file`,
        },
      });
    }

    if (req.method === "POST" && url.pathname.endsWith("/stop")) {
      const id = url.pathname.split("/")[2];
      const job = jobs.get(id);
      if (job && job.proc) {
        job.state = "stopped";
        try { job.proc.kill("SIGTERM"); } catch (e) {}
        activeByAccount.delete(job.account);
      }
      return json(res, 200, { ok: true });
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
      if (data.length > 100_000) reject(new Error("too big"));
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
