/* A gift for a friend without Krate, made from a web build (IC-383).
 *
 * On a desktop, Studio's own engine makes the gift and carries itself
 * inside it. In a tab there is no engine on the sender's side, so this box
 * makes it: the app it built, plus the player for the friend's system.
 *
 * - Mac: a .zip holding the release's notarized opener, the app, and the
 *   release's two signed Mac engines (Apple Silicon and Intel). This box is
 *   Linux and cannot make a disk image or join the two engines into one;
 *   the opener picks the one for the Mac it is on, and installs it only
 *   after checking Krate's Developer ID signed it.
 * - Linux: this box's own engine, the released x86_64 one, rides inside a
 *   single shell file (`krate wrap --for linux` does that by itself here).
 * - Windows: not until Windows builds are signed (K-212).
 *
 * The Mac parts come from the GitHub release of the engine this box runs,
 * checked against that release's SHA256SUMS, fetched once and kept.
 */
import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdir, mkdtemp, readFile, rm, stat, writeFile } from "node:fs/promises";
import { join } from "node:path";

const RELEASES = process.env.KRATE_RELEASE_BASE || "https://github.com/incyashraj/krate/releases/download";

function run(cmd, args, opts = {}) {
  return new Promise((resolve) => {
    const proc = spawn(cmd, args, { ...opts, stdio: ["ignore", "pipe", "pipe"] });
    let out = "", err = "";
    proc.stdout.on("data", (b) => { out += b; });
    proc.stderr.on("data", (b) => { err += b; });
    proc.on("error", (e) => resolve({ code: -1, out, err: String(e && e.message || e) }));
    proc.on("close", (code) => resolve({ code, out, err }));
  });
}

export function createGifts({ krate, stateDir, env = {}, fetchImpl = fetch }) {
  // Every engine call here gets `env`: the service's environment with its
  // secret and the AI keys taken out. Making a gift needs neither, and a
  // child process is not where either belongs.
  const krateRun = (args) => run(krate, args, { env });
  let supported = null; // Promise<boolean>
  let version = null;   // Promise<string|null>
  const kits = new Map(); // version -> Promise<kit>

  /* Whether the engine here can make gifts at all: older releases have no
   * --opener/--player and an opener that installs nothing. The page shows the buttons only when this says yes. */
  function canGift() {
    if (!supported) {
      // The opener script this engine would write: only engines that can
      // carry and install the player mention `player-install` in it.
      supported = krateRun(["gift-opener"]).then((r) => r.code === 0 && /player-install/.test(r.out)).catch(() => false);
    }
    return supported;
  }

  function engineVersion() {
    if (!version) {
      version = krateRun(["--version"]).then((r) => {
        const m = /(\d+\.\d+\.\d+(?:-[0-9A-Za-z.]+)?)/.exec(r.out);
        return m ? m[1] : null;
      });
    }
    return version;
  }

  async function download(url) {
    const r = await fetchImpl(url, { redirect: "follow" });
    if (!r.ok) throw new Error(`could not fetch ${url}: ${r.status}`);
    return Buffer.from(await r.arrayBuffer());
  }

  /* The release's Mac parts, verified and unpacked once per version. */
  function macKit(v) {
    if (!kits.has(v)) {
      const p = (async () => {
        const dir = join(stateDir, "gift-kit", v);
        const ready = join(dir, "ready");
        const kit = {
          opener: join(dir, "opener", "Krate Opener.app"),
          players: [join(dir, "arm64", "krate"), join(dir, "x86_64", "krate")],
        };
        try { await stat(ready); return kit; } catch (e) { /* not yet */ }
        await rm(dir, { recursive: true, force: true });
        await mkdir(dir, { recursive: true });
        const base = `${RELEASES}/v${v}`;
        const sums = (await download(`${base}/SHA256SUMS`)).toString("utf8");
        const want = (name) => {
          const line = sums.split("\n").find((l) => l.trim().endsWith(` ${name}`) || l.trim().endsWith(`*${name}`));
          if (!line) throw new Error(`${name} is not in the release's SHA256SUMS`);
          return line.trim().split(/\s+/)[0];
        };
        const fetchChecked = async (name) => {
          const bytes = await download(`${base}/${name}`);
          const got = createHash("sha256").update(bytes).digest("hex");
          if (got !== want(name)) throw new Error(`${name} does not match the release's checksum`);
          const path = join(dir, name);
          await writeFile(path, bytes);
          return path;
        };
        const openerZip = await fetchChecked(`krate-opener-${v}.zip`);
        await mkdir(join(dir, "opener"), { recursive: true });
        const unz = await run("unzip", ["-q", "-o", openerZip, "-d", join(dir, "opener")]);
        if (unz.code !== 0) throw new Error(`could not unpack the opener: ${unz.err.trim()}`);
        for (const [arch, triple] of [["arm64", "aarch64-apple-darwin"], ["x86_64", "x86_64-apple-darwin"]]) {
          const tgz = await fetchChecked(`krate-${v}-${triple}.tar.gz`);
          const out = join(dir, arch);
          await mkdir(out, { recursive: true });
          const t = await run("tar", ["-xzf", tgz, "-C", out, "--strip-components=1", `krate-${v}-${triple}/krate`]);
          if (t.code !== 0) throw new Error(`could not unpack the ${arch} player: ${t.err.trim()}`);
        }
        await writeFile(ready, new Date().toISOString());
        return kit;
      })();
      // A failure is not kept: the next gift tries again.
      p.catch(() => kits.delete(v));
      kits.set(v, p);
    }
    return kits.get(v);
  }

  /* The gift file for `bytes` (a built .krate) and the friend's system.
   * Returns { bytes, filename } or throws an Error with `status`. */
  async function make(bytes, appName, target) {
    if (target === "windows") {
      const e = new Error("A gift for Windows is not ready yet. Send them the link: they get Krate once from krate.tech, then the app opens.");
      e.status = 400;
      throw e;
    }
    if (target !== "mac" && target !== "linux") {
      const e = new Error("Pick Mac or Linux.");
      e.status = 400;
      throw e;
    }
    if (!(await canGift())) {
      const e = new Error("Gifts from the browser arrive with the next Krate release.");
      e.status = 501;
      throw e;
    }
    const stem = String(appName || "app").replace(/[^A-Za-z0-9]+/g, "-").replace(/^-+|-+$/g, "") || "app";
    const work = await mkdtemp(join(stateDir, "gift-"));
    try {
      const app = join(work, `${stem}.krate`);
      await writeFile(app, bytes);
      let args, out, filename;
      if (target === "mac") {
        const v = await engineVersion();
        if (!v) throw Object.assign(new Error("Krate could not say its version."), { status: 500 });
        const kit = await macKit(v);
        filename = `${stem}-for-Mac.zip`;
        out = join(work, filename);
        args = ["wrap", "--for", "mac", app, "-o", out, "--opener", kit.opener, ...kit.players.flatMap((p) => ["--player", p])];
      } else {
        filename = `${stem}-for-Linux.sh`;
        out = join(work, filename);
        args = ["wrap", "--for", "linux", app, "-o", out];
      }
      const r = await krateRun(args);
      if (r.code !== 0) {
        throw Object.assign(new Error((r.err || r.out).trim().split("\n").pop() || "The gift could not be made."), { status: 500 });
      }
      return { bytes: await readFile(out), filename };
    } finally {
      await rm(work, { recursive: true, force: true });
    }
  }

  /* Fetch the Mac parts ahead of the first gift, so nobody waits on
   * GitHub while their gift is made. Quiet on failure: the gift itself will
   * try again and say what went wrong. */
  async function warm() {
    try {
      if (!(await canGift())) return;
      const v = await engineVersion();
      if (v) await macKit(v);
    } catch (e) { /* the first gift tries again */ }
  }

  return { canGift, make, macKit, warm };
}
