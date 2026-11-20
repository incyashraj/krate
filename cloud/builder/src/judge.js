/* Judging for Krate Cloud's big-file path (IC-945).
 *
 * The hub's Worker has 128 MB of memory and cannot hold a 370 MB app, so
 * the hub asks this box: read the upload from the hub, check its bytes are
 * the ones declared, judge it with `krate judge --tier hub-big` -- the same
 * crates/bundle rules and the same JSON the Worker's own validator gives --
 * and answer. The hub lists nothing until it has that answer.
 *
 * Every call to and from the hub carries the builder secret. The hub asks
 * again for anything still judging, so a failure here only delays.
 */
import { createHash } from "node:crypto";
import { createWriteStream } from "node:fs";
import { mkdir, readFile, rm } from "node:fs/promises";
import { join } from "node:path";
import { spawn } from "node:child_process";
import { Readable } from "node:stream";
import { pipeline } from "node:stream/promises";

/** Run a program; resolve { code, stdout, stderr }. */
//
// `spawnImpl` is the builder's sandbox (K-1293): an upload is a stranger's
// bytes, so the engine that reads them runs as the build user with no
// secret in its environment. The default is for the tests.
//
// The builder's spawnImpl answers a promise (it takes a build user from the
// pool first, K-1297); a plain spawn answers the child. Both are taken.
async function run(program, args, { timeoutMs = 10 * 60 * 1000, spawnImpl = spawn } = {}) {
  let child;
  try {
    child = await spawnImpl(program, args, { stdio: ["ignore", "pipe", "pipe"] });
  } catch (err) {
    return { code: -1, stdout: "", stderr: String(err && err.message || err) };
  }
  return new Promise((resolve) => {
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (d) => (stdout += d));
    child.stderr.on("data", (d) => (stderr += d));
    const timer = setTimeout(() => child.kill("SIGKILL"), timeoutMs);
    // The builder's child says when its build user is back in the pool.
    const back = () => child.handedBack || Promise.resolve();
    child.on("error", (err) => {
      clearTimeout(timer);
      back().then(() => resolve({ code: -1, stdout, stderr: String(err) }));
    });
    child.on("close", (code) => {
      clearTimeout(timer);
      back().then(() => resolve({ code, stdout, stderr }));
    });
  });
}

/** A path inside an archive that cannot land outside where it is unpacked. */
export function safeArchivePath(name) {
  if (!name || name.includes("\\") || name.includes("\0")) return false;
  if (name.startsWith("/") || /^[A-Za-z]:/.test(name)) return false;
  return !name.split("/").some((part) => part === "..");
}

/**
 * Judge one downloaded file. Answers { ok: true, judgement? } or
 * { ok: false, problem }.
 */
export async function judgeFile(kind, file, { krate = "krate", tar = "tar", spawnImpl = spawn } = {}) {
  if (kind === "bundle") {
    const out = await run(krate, ["judge", file, "--tier", "hub-big"], { spawnImpl });
    let answer = null;
    try {
      answer = JSON.parse(out.stdout.trim().split("\n").pop() || "");
    } catch (_) {}
    if (!answer || typeof answer.ok !== "boolean") {
      return { ok: false, problem: `the judge did not answer (${(out.stderr || "").trim().slice(0, 300)})` };
    }
    return answer.ok ? { ok: true, judgement: answer.judgement } : { ok: false, problem: answer.problem };
  }
  if (kind === "source") {
    // A zstd tar whose every entry stays inside the folder it unpacks to.
    const out = await run(tar, ["--zstd", "-tf", file], { spawnImpl });
    if (out.code !== 0) {
      return { ok: false, problem: `not a .tar.zst archive (${(out.stderr || "").trim().slice(0, 200)})` };
    }
    const names = out.stdout.split("\n").filter(Boolean);
    if (names.length === 0) return { ok: false, problem: "the archive is empty" };
    const bad = names.find((n) => !safeArchivePath(n));
    if (bad) return { ok: false, problem: `an unsafe path in the archive: ${bad.slice(0, 120)}` };
    return { ok: true };
  }
  if (kind === "notices") {
    try {
      new TextDecoder("utf-8", { fatal: true }).decode(await readFile(file));
    } catch (_) {
      return { ok: false, problem: "notices are plain UTF-8 text" };
    }
    return { ok: true };
  }
  return { ok: false, problem: `nothing judges a ${kind}` };
}

/**
 * The judging service: `accept(job)` takes { id, kind, size, sha256 } and
 * works in the background; `inFlight` is what it is doing now, so the box
 * does not stop itself mid-judgement.
 */
export function createJudge({ hub, secret, dir, krate = "krate", tar = "tar", spawnImpl = spawn, fetchImpl = fetch, log = console }) {
  const inFlight = new Map();

  async function work(job) {
    const file = join(dir, `${job.id}.bin`);
    try {
      // In production the judge runs as the build user (K-1293): it may
      // read this file by its exact path, and neither list the folder nor
      // swap the file between the hash check and the judging.
      await mkdir(dir, { recursive: true, mode: 0o711 });
      const res = await fetchImpl(`${hub}/internal/big/${job.id}/bytes`, {
        headers: { "x-krate-builder": secret },
      });
      if (!res.ok || !res.body) {
        log.warn(`judge ${job.id}: the hub would not hand over the upload (${res.status})`);
        return;
      }
      // Hash while writing: one pass over 370 MB, never all of it in memory.
      const hash = createHash("sha256");
      let size = 0;
      const source = Readable.fromWeb(res.body);
      source.on("data", (chunk) => {
        hash.update(chunk);
        size += chunk.length;
      });
      await pipeline(source, createWriteStream(file, { mode: 0o644 }));
      const sha256 = hash.digest("hex");
      let verdict;
      if (size !== job.size) {
        verdict = { ok: false, problem: `${size} bytes arrived where ${job.size} were declared` };
      } else if (sha256 !== job.sha256) {
        verdict = { ok: false, problem: "the bytes that arrived are not the ones the upload declared" };
      } else {
        verdict = await judgeFile(job.kind, file, { krate, tar, spawnImpl });
      }
      const answer = await fetchImpl(`${hub}/internal/big/${job.id}/verdict`, {
        method: "POST",
        headers: { "content-type": "application/json", "x-krate-builder": secret },
        body: JSON.stringify({ ...verdict, sha256 }),
      });
      if (!answer.ok) log.warn(`judge ${job.id}: the hub did not take the verdict (${answer.status})`);
    } catch (err) {
      log.warn(`judge ${job.id}: ${err && err.message ? err.message : err}`);
    } finally {
      inFlight.delete(job.id);
      await rm(file, { force: true });
    }
  }

  return {
    inFlight,
    /** Start judging; false when the job is malformed. */
    accept(job) {
      if (!job || !/^[0-9a-zA-Z]{8,40}$/.test(String(job.id || ""))) return false;
      if (!["bundle", "source", "notices"].includes(job.kind)) return false;
      if (!Number.isInteger(job.size) || job.size <= 0) return false;
      if (!/^[0-9a-f]{64}$/.test(String(job.sha256 || ""))) return false;
      if (inFlight.has(job.id)) return true;
      const done = work(job);
      inFlight.set(job.id, done);
      return true;
    },
  };
}
