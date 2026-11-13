/* Approval that shows the code, and approval rules (G15, G16, G22).
 *
 * The bundles are made here, with the source each test needs, and signed
 * for the organisation by the validator's own signing code under a root
 * key made for the test (org-bundle-tools.mjs); every upload goes through
 * the real door and the real validator.
 *
 *   node --experimental-wasm-modules --test cloud/worker/test/org-approvals.test.mjs
 */
import { test } from "node:test";
import assert from "node:assert";
import { orgEnv, call, acme } from "./org-tools.mjs";
import { rootKey, bundle, COMPONENT } from "./org-bundle-tools.mjs";
import { diffLines, hunksOf, TOTAL_DIFF_MAX, FILE_DIFF_MAX } from "../src/code-changes.js";
import { needsWords } from "../src/org-approvals.js";

const pair = (r) => [r.status, r.body && r.body.error];
const NET = ["net.connect:api.acme.example:443", "send expenses to finance"];

/* acme (alice owner, bob reviewer, carol developer, dan recipient) with
 * erin an admin and frank a second reviewer. */
async function world() {
  const key = rootKey();
  const env = orgEnv();
  await acme(env, { root: key.public, revocations: null });
  for (const [login, role] of [["erin", "admin"], ["frank", "reviewer"]]) {
    const inv = await call(env, "alice", "POST", "/orgs/acme/invites", { json: { github: login, role } });
    assert.strictEqual((await call(env, login, "POST", `/orgs/acme/invites/${inv.body.code}/accept`)).status, 200);
  }
  return { env, key };
}
const send = async (env, bytes, who = "carol") => {
  const up = await call(env, who, "POST", "/orgs/acme/candidates", { bytes });
  assert.strictEqual(up.status, 201, JSON.stringify(up.body));
  return up.body.digest;
};
const approve = (env, who, digest, accept) =>
  call(env, who, "POST", `/orgs/acme/candidates/${digest}/approve`, { json: accept ? { accept } : {} });
const changes = (env, who, digest) => call(env, who, "GET", `/orgs/acme/candidates/${digest}/changes`);
const listed = async (env, who, digest) => (await call(env, who, "GET", "/orgs/acme/candidates")).body.find((c) => c.digest === digest);
const rules = (env, who, policy) => call(env, who, "PUT", "/orgs/acme/settings/approvals", { json: policy });
const group = async (env, name, ...logins) => {
  assert.strictEqual((await call(env, "alice", "POST", "/orgs/acme/groups", { json: { name } })).status, 201);
  for (const l of logins) assert.strictEqual((await call(env, "alice", "PUT", `/orgs/acme/groups/${name}/members/${encodeURIComponent(`github:${l}`)}`)).status, 200);
};
const audit = async (env) => (await call(env, "alice", "GET", "/orgs/acme/audit")).body.events;

/* The compiled app with `extra` bytes more, in a custom section: still a
 * valid component, so the validator takes it. */
function grown(extra) {
  const name = new TextEncoder().encode("padding");
  const body = [name.length, ...name, ...new Uint8Array(extra)];
  const leb = [];
  let n = body.length;
  do {
    let b = n & 0x7f;
    n >>>= 7;
    if (n) b |= 0x80;
    leb.push(b);
  } while (n);
  return new Uint8Array([...COMPONENT, 0, ...leb, ...body]);
}

const LOCK = (pkgs) =>
  `version = 3\n\n[[package]]\nname = "bounce"\nversion = "0.1.0"\n` +
  pkgs.map(([n, v, src = "registry+https://github.com/rust-lang/crates.io-index"]) => `\n[[package]]\nname = "${n}"\nversion = "${v}"\nsource = "${src}"\n`).join("");

const V1 = {
  "Cargo.toml": '[package]\nname = "bounce"\nversion = "0.1.0"\n\n[dependencies]\nserde = "1.0"\n',
  "Cargo.lock": LOCK([["serde", "1.0.200"], ["itoa", "1.0.9"]]),
  "src/lib.rs": "fn draw() {\n    let x = 1;\n    let y = 2;\n    let z = 3;\n    paint(x, y, z);\n}\n",
  "src/old.rs": "pub fn gone() {}\n",
  "README.md": "Bounce.\n",
};
const V2 = {
  "Cargo.toml": '[package]\nname = "bounce"\nversion = "0.1.0"\n\n[dependencies]\nserde = "1.0"\nserde_json = "1.0"\n',
  "Cargo.lock": LOCK([["serde", "1.0.210"], ["itoa", "1.0.9"], ["serde_json", "1.0.128"]]),
  "src/lib.rs": "fn draw() {\n    let x = 1;\n    let y = 20;\n    let z = 3;\n    paint(x, y, z);\n}\n",
  "src/net.rs": "pub fn send() {\n    unsafe { raw_send() }\n    unsafe { raw_flush() }\n}\n",
  "README.md": "Bounce.\n",
};

test("what changed in the code: files, an exact line diff, and the risk signals, against the release it replaces", async () => {
  const { env, key } = await world();
  const d1 = await send(env, bundle(key, { version: "1.0.0", source: V1 }));
  assert.strictEqual((await approve(env, "bob", d1)).status, 200);
  const blob = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0, 0, 0, 1, 2, 3]);
  const d2 = await send(env, bundle(key, { version: "1.1.0", source: { ...V2, "assets/blob.bin": blob }, caps: [NET], component: grown(80 * 1024) }));

  const r = await changes(env, "bob", d2);
  assert.strictEqual(r.status, 200, JSON.stringify(r.body));
  assert.strictEqual(r.headers.get("cache-control"), "private, no-store");
  assert.deepStrictEqual(r.body.against, { digest: d1, version: "1.0.0" });
  assert.strictEqual(r.body.new_app, false);
  const by = Object.fromEntries(r.body.files.map((f) => [f.path, f]));
  // Unchanged files are counted, not listed; Cargo.lock comes last.
  assert.strictEqual(r.body.unchanged, 1);
  assert.deepStrictEqual(r.body.files.map((f) => [f.path, f.status]), [
    ["Cargo.toml", "changed"], ["assets/blob.bin", "added"], ["src/lib.rs", "changed"], ["src/net.rs", "added"], ["src/old.rs", "removed"], ["Cargo.lock", "changed"],
  ]);
  // The one changed line, with its three lines of context, numbered as a unified diff.
  assert.deepStrictEqual(by["src/lib.rs"].hunks, [{
    old_start: 1, old_lines: 6, new_start: 1, new_lines: 6,
    lines: [" fn draw() {", "     let x = 1;", "-    let y = 2;", "+    let y = 20;", "     let z = 3;", "     paint(x, y, z);", " }"],
  }]);
  assert.deepStrictEqual([by["src/lib.rs"].added_lines, by["src/lib.rs"].removed_lines], [1, 1]);
  assert.deepStrictEqual(by["src/old.rs"].hunks, [{ old_start: 1, old_lines: 1, new_start: 0, new_lines: 0, lines: ["-pub fn gone() {}"] }]);
  assert.strictEqual(by["src/net.rs"].hunks[0].lines.length, 4);
  assert.strictEqual(by["assets/blob.bin"].binary, true);
  assert.deepStrictEqual(by["assets/blob.bin"].hunks, []);
  assert.deepStrictEqual(r.body.totals, { files: 6, added_lines: by["Cargo.toml"].added_lines + 1 + 4 + by["Cargo.lock"].added_lines, removed_lines: 1 + 1 + by["Cargo.lock"].removed_lines });
  assert.strictEqual(r.body.truncated, false);

  const risks = r.body.risks.map((x) => x.text);
  for (const want of [
    "New network host: api.acme.example:443",
    "New dependency: serde_json 1.0.128",
    "Dependency changed: serde 1.0.200 to 1.0.210",
    "2 new unsafe blocks in src/net.rs",
    "New binary file: assets/blob.bin (10 bytes)",
    "Made by hand",
  ]) assert.ok(risks.includes(want), `${want} in ${JSON.stringify(risks)}`);
  assert.ok(risks.some((t) => /^The compiled app grew from 30\.2 KB to 110\.2 KB \(\+265%\)$/.test(t)), JSON.stringify(risks));
  assert.strictEqual(r.body.risks.find((x) => x.kind === "size").level, "high");
  assert.strictEqual(r.body.code.size - r.body.code.old_size > 80 * 1024, true);

  // Kept beside the bytes, and answered from there the second time.
  assert.ok(env.BUNDLES._blobs.has(`changes:${d2}`), "the comparison is cached");
  const again = await changes(env, "alice", d2);
  assert.deepStrictEqual(again.body.files, r.body.files);
  // No source is ever written into the audit.
  assert.ok(!JSON.stringify(await audit(env)).includes("raw_send"));
});

test("a new app: every file is new, and a version with no source says so", async () => {
  const { env, key } = await world();
  const d1 = await send(env, bundle(key, { version: "1.0.0", source: V1 }));
  const r = await changes(env, "bob", d1);
  assert.strictEqual(r.status, 200);
  assert.strictEqual(r.body.new_app, true);
  assert.strictEqual(r.body.against, null);
  assert.ok(r.body.files.every((f) => f.status === "added"));
  assert.strictEqual(r.body.files.length, 5);
  assert.ok(r.body.risks.some((x) => x.text === "Depends on 1 crate: serde 1.0"), JSON.stringify(r.body.risks));
  assert.ok(r.body.risks.some((x) => x.text === "2 crates in all, from Cargo.lock"));

  const bare = await send(env, bundle(key, { version: "1.0.1" }));
  const b = await changes(env, "bob", bare);
  assert.strictEqual(b.status, 200);
  assert.strictEqual(b.body.files.length, 0);
  assert.strictEqual(b.body.risks[0].kind, "no-source");
  assert.strictEqual(b.body.risks[0].level, "high");
});

test("a big change is cut at the bounds and says so, file by file", async () => {
  const { env, key } = await world();
  const line = (i, w) => `let value_${i} = "${w.repeat(60)}";`;
  const big = (w) => Array.from({ length: 4000 }, (_, i) => line(i, w)).join("\n") + "\n";
  const source = {};
  for (let i = 0; i < 9; i += 1) source[`src/f${i}.rs`] = big("a");
  const d1 = await send(env, bundle(key, { version: "1.0.0", source }));
  assert.strictEqual((await approve(env, "bob", d1)).status, 200);
  const next = {};
  for (let i = 0; i < 9; i += 1) next[`src/f${i}.rs`] = big("b");
  const d2 = await send(env, bundle(key, { version: "1.1.0", source: next }));
  const r = await changes(env, "bob", d2);
  assert.strictEqual(r.status, 200);
  assert.strictEqual(r.body.truncated, true);
  const size = (f) => f.hunks.reduce((s, h) => s + h.lines.reduce((t, l) => t + l.length + 1, 0), 0);
  const total = r.body.files.reduce((s, f) => s + size(f), 0);
  assert.ok(total <= TOTAL_DIFF_MAX, `${total} within ${TOTAL_DIFF_MAX}`);
  assert.ok(r.body.files.every((f) => size(f) <= FILE_DIFF_MAX));
  assert.ok(r.body.files[0].truncated, "the first file is cut at its own bound");
  assert.ok(r.body.files.some((f) => f.omitted && !f.hunks.length), "a file past the total bound is listed with no diff");
  // Every file is still listed with its true counts.
  assert.strictEqual(r.body.files.length, 9);
  assert.ok(r.body.files.every((f) => f.added_lines === 4000 && f.removed_lines === 4000));
});

test("only the people who approve, and the version's sender, read its code", async () => {
  const { env, key } = await world();
  const d1 = await send(env, bundle(key, { version: "1.0.0", source: V1 }));
  // frank is a developer here: one who did not send it.
  assert.strictEqual((await call(env, "alice", "PUT", `/orgs/acme/members/${encodeURIComponent("github:frank")}`, { json: { role: "developer" } })).status, 200);
  assert.strictEqual((await changes(env, "alice", d1)).status, 200, "an owner");
  assert.strictEqual((await changes(env, "bob", d1)).status, 200, "a reviewer");
  assert.strictEqual((await changes(env, "carol", d1)).status, 200, "its sender");
  const member = await changes(env, "dan", d1);
  assert.deepStrictEqual(pair(member), [403, "role-cannot"]);
  assert.match(member.body.message, /only the people who approve versions in Acme, and whoever sent a version, read its code/);
  assert.ok(!JSON.stringify(member.body).includes("fn draw"), "a refusal carries no code");
  assert.deepStrictEqual(pair(await changes(env, "erin", d1)), [403, "role-cannot"], "an admin does not approve");
  assert.deepStrictEqual(pair(await changes(env, "frank", d1)), [403, "role-cannot"], "a developer who did not send it");
  assert.deepStrictEqual(pair(await changes(env, "stranger", d1)), [403, "not-a-member"]);
  assert.deepStrictEqual(pair(await changes(env, null, d1)), [401, "not-signed-in"]);
  assert.deepStrictEqual(pair(await changes(env, "bob", "f".repeat(64))), [404, "digest-mismatch"]);
  const refused = (await audit(env)).filter((e) => e.action === "candidate.changes" && e.result === "denied" && e.reason === "role-cannot");
  assert.deepStrictEqual(refused.map((e) => e.actor).sort(), ["github:dan", "github:erin", "github:frank"]);
});

test("the default rules approve exactly as before, and only an owner changes them", async () => {
  const { env, key } = await world();
  const show = await call(env, "dan", "GET", "/orgs/acme/settings/approvals");
  assert.strictEqual(show.status, 200);
  assert.strictEqual(show.body.default, true);
  assert.deepStrictEqual(show.body.policy, { approvals: 1, sensitive_approvals: 1, reviewer_groups: [], security_group: null });
  const d1 = await send(env, bundle(key, { version: "1.0.0", source: V1 }));
  const c = await listed(env, "bob", d1);
  assert.deepStrictEqual([c.approval.rules, c.approval.needed, c.approval.words, c.approval.you.can], [false, 1, "Needs one approval", true]);
  assert.strictEqual((await approve(env, "bob", d1)).status, 200);

  assert.deepStrictEqual(pair(await rules(env, "bob", { approvals: 2 })), [403, "role-cannot"]);
  assert.deepStrictEqual(pair(await rules(env, "erin", { approvals: 2 })), [403, "role-cannot"]);
  const bad = await rules(env, "alice", { approvals: 3 });
  assert.deepStrictEqual(pair(bad), [400, "policy-invalid"]);
  assert.match(bad.body.message, /approvals is 1 or 2 approvals/);
  const nogroup = await rules(env, "alice", { security_group: "security" });
  assert.deepStrictEqual(pair(nogroup), [400, "policy-invalid"]);
  assert.match(nogroup.body.message, /there is no group called security -- make it under People first/);
  const set = await rules(env, "alice", { approvals: 2 });
  assert.strictEqual(set.status, 200);
  assert.deepStrictEqual([set.body.default, set.body.words], [false, "two approvals for every version"]);
  const ev = (await audit(env)).filter((e) => e.action === "settings.approvals");
  assert.deepStrictEqual(ev.map((e) => [e.actor, e.result, e.reason]), [["github:alice", "allowed", "two approvals for every version"]]);
  // The organisation's own view carries the rules.
  assert.strictEqual((await call(env, "dan", "GET", "/orgs/acme")).body.org.approval_policy.default, false);
});

test("two approvals: the first is recorded and audited, the second releases, nobody approves twice", async () => {
  const { env, key } = await world();
  assert.strictEqual((await rules(env, "alice", { approvals: 2 })).status, 200);
  const d1 = await send(env, bundle(key, { version: "1.0.0", source: V1 }));
  const first = await approve(env, "bob", d1);
  assert.strictEqual(first.status, 202, JSON.stringify(first.body));
  assert.deepStrictEqual([first.body.state, first.body.approval.words, first.body.approval.approvals.map((a) => a.display)], ["waiting-for-approval", "Needs one more approval", ["bob"]]);
  assert.strictEqual((await listed(env, "alice", d1)).state, "waiting-for-approval");
  // Nothing opens yet.
  assert.deepStrictEqual(pair(await call(env, "dan", "GET", `/a/${d1}`)), [403, "digest-mismatch"]);
  const twice = await approve(env, "bob", d1);
  assert.deepStrictEqual(pair(twice), [409, "already-approved"]);
  assert.match(twice.body.message, /you approved Bounce 1\.0\.0 already -- it needs one more approval/);
  assert.deepStrictEqual(pair(await approve(env, "carol", d1)), [403, "self-approval"]);
  // bob sees he is done; frank that he can finish it.
  const forBob = (await listed(env, "bob", d1)).approval;
  assert.deepStrictEqual([forBob.you.approved, forBob.you.can, forBob.you.why], [true, false, "you approved it"]);
  assert.strictEqual((await listed(env, "frank", d1)).approval.you.can, true);
  const second = await approve(env, "frank", d1);
  assert.strictEqual(second.status, 200, JSON.stringify(second.body));
  assert.strictEqual(second.body.release.digest, d1);
  assert.strictEqual((await listed(env, "alice", d1)).state, "released");
  assert.deepStrictEqual((await listed(env, "alice", d1)).approved_with, ["github:bob", "github:frank"]);
  const ev = (await audit(env)).filter((e) => e.target === `candidate:${d1}` && e.result === "allowed" && /^candidate\.approve/.test(e.action));
  assert.deepStrictEqual(ev.map((e) => [e.action, e.actor, e.reason]), [
    ["candidate.approve.partial", "github:bob", "1 of 2 -- needs one more approval"],
    ["candidate.approve", "github:frank", "approval rules met, with bob"],
  ]);
  // Refusals were audited too.
  assert.ok((await audit(env)).some((e) => e.action === "candidate.approve" && e.result === "denied" && e.reason === "already-approved"));
});

test("a stricter count only for a version that asks for the network, files or the camera, or adds a capability", async () => {
  const { env, key } = await world();
  assert.strictEqual((await rules(env, "alice", { approvals: 1, sensitive_approvals: 2 })).status, 200);
  const plain = await send(env, bundle(key, { version: "1.0.0", source: V1 }));
  assert.strictEqual((await listed(env, "bob", plain)).approval.needed, 1);
  assert.strictEqual((await approve(env, "bob", plain)).status, 200, "nothing sensitive: one approval");
  const net = await send(env, bundle(key, { version: "1.1.0", source: V1, caps: [NET] }));
  const c = await listed(env, "bob", net);
  assert.deepStrictEqual([c.approval.sensitive, c.approval.needed, c.approval.words], [true, 2, "Needs 2 approvals"]);
  const accept = [NET[0]];
  assert.strictEqual((await approve(env, "bob", net, accept)).status, 202);
  // The capability check still applies to a partial approval.
  assert.deepStrictEqual(pair(await approve(env, "frank", net)), [409, "new-capabilities"]);
  assert.strictEqual((await approve(env, "frank", net, accept)).status, 200);
});

test("a reviewer group: an approver outside it cannot be the one that releases", async () => {
  const { env, key } = await world();
  await group(env, "finance", "frank");
  assert.strictEqual((await rules(env, "alice", { reviewer_groups: [{ app: "dev.krate.bounce", group: "finance" }] })).status, 200);
  const d1 = await send(env, bundle(key, { version: "1.0.0", source: V1 }));
  const c = await listed(env, "bob", d1);
  assert.strictEqual(c.approval.words, "Needs an approval from finance");
  assert.deepStrictEqual([c.approval.you.can, c.approval.you.why], [false, "it needs an approval from finance, and you are not in that group"]);
  const no = await approve(env, "bob", d1);
  assert.deepStrictEqual(pair(no), [403, "approval-policy"]);
  assert.match(no.body.message, /Acme's approval rules need an approval from finance for Bounce 1\.0\.0 \(every version of Bounce needs one\), and you are not in that group -- ask someone in finance to approve it/);
  assert.strictEqual((await listed(env, "alice", d1)).state, "waiting-for-approval");
  assert.strictEqual((await approve(env, "frank", d1)).status, 200);
  // Another app is not this rule's.
  assert.strictEqual(needsWords(1, 0, []), "Needs one approval");
});

test("a new network host needs an approver from the security group, with two approvals", async () => {
  const { env, key } = await world();
  await group(env, "security", "alice");
  assert.strictEqual((await rules(env, "alice", { approvals: 2, security_group: "security" })).status, 200);
  const d1 = await send(env, bundle(key, { version: "1.0.0", source: V1 }));
  // No host: two approvals from anyone.
  assert.strictEqual((await approve(env, "bob", d1)).status, 202);
  assert.strictEqual((await approve(env, "frank", d1)).status, 200);
  const d2 = await send(env, bundle(key, { version: "1.1.0", source: V2, caps: [NET] }));
  const accept = [NET[0]];
  let c = await listed(env, "bob", d2);
  assert.strictEqual(c.approval.words, "Needs an approval from security and one more from anyone who approves");
  assert.deepStrictEqual(c.approval.groups, [{ group: "security", why: "it connects to a new host: api.acme.example:443", met: false }]);
  assert.strictEqual((await approve(env, "bob", d2, accept)).status, 202);
  c = await listed(env, "frank", d2);
  assert.strictEqual(c.approval.words, "Needs one more approval from security");
  assert.strictEqual(c.approval.you.can, false);
  const no = await approve(env, "frank", d2, accept);
  assert.deepStrictEqual(pair(no), [403, "approval-policy"]);
  assert.match(no.body.message, /it connects to a new host: api\.acme\.example:443/);
  const yes = await approve(env, "alice", d2, accept);
  assert.strictEqual(yes.status, 200, JSON.stringify(yes.body));
  assert.strictEqual((await listed(env, "alice", d2)).state, "released");
});

test("a removed approver no longer counts, and a version waits only on those who can still help", async () => {
  const { env, key } = await world();
  assert.strictEqual((await rules(env, "alice", { approvals: 2 })).status, 200);
  const d1 = await send(env, bundle(key, { version: "1.0.0", source: V1 }));
  assert.strictEqual((await approve(env, "frank", d1)).status, 202);
  // frank no longer sees it as waiting for him.
  const notes = (await call(env, "frank", "GET", "/orgs/acme/notifications")).body.items;
  assert.ok(!notes.some((n) => n.kind === "approval-waiting" && n.target === d1), JSON.stringify(notes));
  assert.ok((await call(env, "bob", "GET", "/orgs/acme/notifications")).body.items.some((n) => n.kind === "approval-waiting" && n.target === d1));
  assert.strictEqual((await call(env, "alice", "DELETE", `/orgs/acme/members/${encodeURIComponent("github:frank")}`)).status, 204);
  assert.strictEqual((await listed(env, "alice", d1)).approval.approvals.length, 0);
  assert.strictEqual((await approve(env, "bob", d1)).status, 202, "bob's is the first that counts now");
  assert.strictEqual((await approve(env, "alice", d1)).status, 200);
});

test("solo release stays as it was under the default rules, and stops under any other", async () => {
  const key = rootKey();
  const env = orgEnv();
  assert.strictEqual((await call(env, "alice", "POST", "/orgs", { json: { slug: "acme", name: "Acme" } })).status, 201);
  assert.strictEqual((await call(env, "alice", "PUT", "/orgs/acme/root-key", { json: { public_key: key.public } })).status, 200);
  const d1 = await send(env, bundle(key, { version: "1.0.0", source: V1 }), "alice");
  const solo = await approve(env, "alice", d1);
  assert.strictEqual(solo.status, 200, JSON.stringify(solo.body));
  assert.strictEqual(solo.body.release.solo, true);
  const set = await rules(env, "alice", { approvals: 2 });
  assert.strictEqual(set.status, 200);
  assert.match(set.body.warnings.join(" "), /Only one person can approve, so a version that needs 2 approvals waits until more join/);
  assert.strictEqual((await call(env, "alice", "GET", "/orgs/acme")).body.org.release_rule, "two-person");
  const d2 = await send(env, bundle(key, { version: "1.1.0", source: V1 }), "alice");
  assert.deepStrictEqual(pair(await approve(env, "alice", d2)), [403, "self-approval"]);
  // Back to the default: solo release again.
  assert.strictEqual((await rules(env, "alice", { approvals: 1 })).status, 200);
  assert.strictEqual((await approve(env, "alice", d2)).body.release.solo, true);
});

test("the waiting mail links straight to the version", async () => {
  const { env, key } = await world();
  const d1 = await send(env, bundle(key, { version: "1.0.0", source: V1 }));
  // The plan the door mails from, asked of the store as the door asks it.
  const plan = env.ORGS.object("acme").run({ op: "notify-plan", slug: "acme", principals: [], args: { event: "waiting", digest: d1, site: "https://krate.tech" } });
  const mail = plan.body.mails.find((m) => m.principal === "github:bob");
  assert.ok(mail, JSON.stringify(plan.body));
  assert.ok(mail.text.includes(`See its code and what it can do, then approve or decline it: https://krate.tech/org/?o=acme&tab=waiting&c=${d1}`), mail.text);
});

test("the line diff is exact on edits anywhere, and its hunks number lines as unified diffs do", () => {
  for (let t = 0; t < 200; t += 1) {
    const r = () => Array.from({ length: Math.floor(Math.random() * 40) }, () => "abcd"[Math.floor(Math.random() * 4)]);
    const a = r();
    const b = r();
    const ops = diffLines(a, b);
    assert.deepStrictEqual(ops.filter((o) => o[0] !== "+").map((o) => o[1]), a);
    assert.deepStrictEqual(ops.filter((o) => o[0] !== "-").map((o) => o[1]), b);
    for (const h of hunksOf(ops)) {
      assert.strictEqual(h.lines.filter((l) => l[0] !== "+").length, h.old_lines);
      assert.strictEqual(h.lines.filter((l) => l[0] !== "-").length, h.new_lines);
    }
  }
});
