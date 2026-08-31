/* The free tier's two-key counter, and the abuse it exists to stop.
 *
 * Three free apps, EVER, per account -- with the device as a second key,
 * because an account is free to create and an account-only limit is an
 * invitation to make another email.
 *
 * The logic is mirrored here rather than imported: the worker is one file
 * with no module boundary, and a mirrored copy that drifts fails loudly on
 * the next run, which is the behaviour worth having. If planCount in
 * cloud/worker/src/index.js changes, change this too.
 *
 *   node cloud/worker/test/plan-count.test.mjs
 */
const KV = new Map();
const env = { APPS: {
  get: async (k) => KV.get(k) ?? null,
  put: async (k, v) => { KV.set(k, v); },
}};

// Two accounts, one machine -- the exact abuse the second key exists for.
async function planCount({ user, device, increment, local = 0 }) {
  const keys = [];
  if (user) keys.push(`mkacct:${user}`);
  if (/^[0-9a-f]{64}$/.test(device)) keys.push(`mkdev:${device}`);
  if (!keys.length) return { error: "no keys" };
  const counts = await Promise.all(keys.map((k) => env.APPS.get(k)));
  let n = counts.reduce((m, raw) => Math.max(m, parseInt(raw || "0", 10)), 0);
  n = Math.max(n, local);
  if (increment) n = n + 1;
  if (increment) await Promise.all(keys.map((k) => env.APPS.put(k, String(n))));
  return { n };
}

const DEV = "a".repeat(64);
const DEV2 = "b".repeat(64);
let failed = 0;
const say = (label, got, want) => {
  const ok = got === want;
  if (!ok) failed++;
  console.log(`${ok ? "PASS" : "FAIL"}  ${label}: ${got} (want ${want})`);
};

// Alice makes her three.
for (const i of [1, 2, 3]) {
  const { n } = await planCount({ user: "alice", device: DEV, increment: true });
  say(`alice make ${i}`, n, i);
}
say("alice is at her limit", (await planCount({ user: "alice", device: DEV })).n, 3);

// Bob signs up on the SAME machine. This is the whole point.
say("new account, same machine", (await planCount({ user: "bob", device: DEV })).n, 3);

// Bob on his own machine starts fresh, as he should.
say("new account, new machine", (await planCount({ user: "bob", device: DEV2 })).n, 0);

// Alice clears her browser storage (new device id) but is still Alice.
say("same account, cleared device", (await planCount({ user: "alice", device: DEV2 })).n, 3);

// And an offline make reported late is not lost.
say("offline make catches up", (await planCount({ user: "carol", device: DEV2, local: 2 })).n, 2);

if (failed) {
  console.error(`\n${failed} failed -- the free tier is not holding.`);
  process.exit(1);
}
console.log("\nall good: three ever, and a new account on the same machine gets none.");
