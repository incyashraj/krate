/* The build service's half of the big-file path (IC-945): read an upload
 * from the hub, check it is the declared bytes, judge it, answer.
 *
 * A fake hub on a local port plays the Worker's two builder-only routes;
 * the judge is `krate judge` itself when a built binary is at hand
 * (target/debug/krate or KRATE_BIN), and a stand-in otherwise.
 *
 *   node cloud/builder/test/judge.test.mjs
 */
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { createHash } from "node:crypto";
import { existsSync } from "node:fs";
import { chmod, mkdtemp, readFile, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { execFileSync } from "node:child_process";
import { createJudge, judgeFile, safeArchivePath } from "../src/judge.js";

const SECRET = "test-secret";
const work = await mkdtemp(join(tmpdir(), "krate-judge-test-"));
const sha = (b) => createHash("sha256").update(b).digest("hex");

/* ---- paths an archive may and may not carry ---------------------------- */
for (const good of ["a.txt", "veloren/Cargo.toml", "krate-sdk/wit/x.wit", "dir/"]) {
  assert.ok(safeArchivePath(good), good);
}
for (const bad of ["/etc/passwd", "../x", "a/../../x", "C:/x", "a\\b", ""]) {
  assert.ok(!safeArchivePath(bad), bad);
}

/* ---- a fake hub: hands over one upload, records the verdict ------------- */
function fakeHub(uploads) {
  const verdicts = [];
  const server = createServer(async (req, res) => {
    if (req.headers["x-krate-builder"] !== SECRET) {
      res.writeHead(404).end("not found");
      return;
    }
    const m = /^\/internal\/big\/([0-9a-zA-Z]+)\/(bytes|verdict)$/.exec(req.url);
    if (m && m[2] === "bytes" && uploads.has(m[1])) {
      const bytes = uploads.get(m[1]);
      res.writeHead(200, { "content-length": String(bytes.length) }).end(bytes);
      return;
    }
    if (m && m[2] === "verdict") {
      let body = "";
      for await (const chunk of req) body += chunk;
      verdicts.push({ id: m[1], ...JSON.parse(body) });
      res.writeHead(200).end("{}");
      return;
    }
    res.writeHead(404).end("not found");
  });
  return new Promise((done) =>
    server.listen(0, "127.0.0.1", () =>
      done({ base: `http://127.0.0.1:${server.address().port}`, verdicts, close: () => server.close() }),
    ),
  );
}

const settle = (judge) => Promise.all([...judge.inFlight.values()]);

/* ---- a stand-in krate: its answer is whatever the test wrote ----------- */
const fakeKrate = join(work, "fake-krate");
await writeFile(fakeKrate, `#!/bin/sh\necho "$@" >> "${join(work, "args.log")}"\ncat "${join(work, "answer.json")}"\n`);
await chmod(fakeKrate, 0o755);

{
  const bytes = Buffer.from("PK pretend this is a bundle");
  const hub = await fakeHub(new Map([["job1aaaa", bytes], ["job2aaaa", bytes], ["job3aaaa", bytes]]));
  const judge = createJudge({ hub: hub.base, secret: SECRET, dir: join(work, "j"), krate: fakeKrate, log: { warn() {} } });
  await writeFile(join(work, "answer.json"), JSON.stringify({ ok: true, judgement: { manifest: { app: { id: "x" } } } }));

  // Malformed jobs are refused before anything is fetched.
  assert.equal(judge.accept({ id: "no", kind: "bundle", size: 1, sha256: "a".repeat(64) }), false);
  assert.equal(judge.accept({ id: "job1aaaa", kind: "spell", size: 1, sha256: "a".repeat(64) }), false);

  // The declared bytes: judged, and the judge's answer goes back.
  assert.equal(judge.accept({ id: "job1aaaa", kind: "bundle", size: bytes.length, sha256: sha(bytes) }), true);
  await settle(judge);
  assert.equal(hub.verdicts.length, 1);
  assert.equal(hub.verdicts[0].ok, true);
  assert.equal(hub.verdicts[0].sha256, sha(bytes));
  assert.equal(hub.verdicts[0].judgement.manifest.app.id, "x");

  // Bytes that are not the declared ones are refused without judging.
  await writeFile(join(work, "answer.json"), "this must not be read");
  judge.accept({ id: "job2aaaa", kind: "bundle", size: bytes.length, sha256: "0".repeat(64) });
  await settle(judge);
  assert.equal(hub.verdicts[1].ok, false);
  assert.match(hub.verdicts[1].problem, /not the ones the upload declared/);
  judge.accept({ id: "job3aaaa", kind: "bundle", size: bytes.length + 1, sha256: sha(bytes) });
  await settle(judge);
  assert.match(hub.verdicts[2].problem, /bytes arrived where/);

  // The file it judged was not at a path the upload's id alone names: a
  // build user that learns an id cannot open the upload (K-1297).
  const judged = (await readFile(join(work, "args.log"), "utf8")).trim().split(/\s+/).find((a) => a.includes("job1aaaa"));
  assert.ok(judged, "the engine was handed the upload");
  assert.match(judged, /job1aaaa-[0-9a-f]{32}\.bin$/, `an unguessable name: ${judged}`);

  // Nothing is left on the box.
  assert.ok(!existsSync(judged));
  assert.ok(!existsSync(join(work, "j", "job1aaaa.bin")));
  hub.close();
}

/* ---- a judge that says nothing readable is a refusal, not a pass ------- */
{
  await writeFile(join(work, "answer.json"), "garbage");
  const out = await judgeFile("bundle", join(work, "answer.json"), { krate: fakeKrate });
  assert.equal(out.ok, false);
  assert.match(out.problem, /did not answer/);
}

/* ---- source: a zstd tar whose paths stay inside it --------------------- */
{
  const dir = join(work, "src");
  execFileSync("mkdir", ["-p", join(dir, "veloren")]);
  await writeFile(join(dir, "veloren", "README.md"), "hi");
  const good = join(work, "good.tar.zst");
  execFileSync("tar", ["--zstd", "-cf", good, "-C", dir, "veloren"]);
  assert.deepEqual(await judgeFile("source", good), { ok: true });
  assert.equal((await judgeFile("source", join(work, "answer.json"))).ok, false, "not an archive");
  // An entry that climbs out, as a listing would show it.
  const fakeTar = join(work, "fake-tar");
  await writeFile(fakeTar, "#!/bin/sh\nprintf 'veloren/ok\\n../../etc/cron.d/x\\n'\n");
  await chmod(fakeTar, 0o755);
  const climbs = await judgeFile("source", good, { tar: fakeTar });
  assert.equal(climbs.ok, false);
  assert.match(climbs.problem, /unsafe path/);
}

/* ---- notices: UTF-8 text ------------------------------------------------ */
{
  const text = join(work, "notices.txt");
  await writeFile(text, "Music: GeekyGami, CC BY-SA 4.0 — thank you");
  assert.deepEqual(await judgeFile("notices", text), { ok: true });
  const bin = join(work, "notices.bin");
  await writeFile(bin, Buffer.from([0xff, 0xfe, 0x00, 0xc3]));
  assert.equal((await judgeFile("notices", bin)).ok, false);
}

/* ---- the real judge, when a built krate is at hand ---------------------- */
{
  const krate = process.env.KRATE_BIN || resolve("target/debug/krate");
  if (existsSync(krate)) {
    const dir = join(work, "real");
    execFileSync("mkdir", ["-p", dir]);
    await writeFile(
      join(dir, "manifest.toml"),
      '[app]\nid = "dev.krate.judged"\nname = "Judged"\nversion = "1.0.0"\nentry = "code.wasm"\nworld = "krate:app/cli@0.1.0"\n',
    );
    execFileSync("cp", [resolve("crates/bundle/tests/fixtures/minimal-run.wasm"), join(dir, "code.wasm")]);
    const bundle = join(dir, "judged.krate");
    execFileSync(krate, ["pack", join(dir, "code.wasm"), "--manifest", join(dir, "manifest.toml"), "-o", bundle], {
      stdio: "ignore",
    });
    const out = await judgeFile("bundle", bundle, { krate });
    assert.equal(out.ok, true, JSON.stringify(out));
    assert.equal(out.judgement.manifest.app.id, "dev.krate.judged");
    const junk = join(dir, "junk.krate");
    await writeFile(junk, "not a zip");
    const refused = await judgeFile("bundle", junk, { krate });
    assert.equal(refused.ok, false);
  } else {
    console.log("judge: no built krate here; the real-judge case stood down");
  }
}

console.log("judge: ok");
