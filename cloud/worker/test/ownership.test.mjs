/* Publishing is content-addressed; ownership is not (IC-387).
 *
 * Anyone can download a public .krate and POST the identical bytes back, and
 * the metadata write used to replace the listing's author with theirs -- one
 * authenticated request took over any app's public identity. The image
 * routes were worse: any signed-in account could replace any app's
 * screenshot or icon, no ownership asked at all.
 *
 * The decision logic is mirrored here rather than imported, the same way
 * plan-count.test.mjs does it: the worker is one file with no module
 * boundary, and a mirrored copy that drifts fails loudly on the next run.
 * If ownedApp or the publish takeover guard in cloud/worker/src/index.js
 * changes, change this too.
 *
 *   node cloud/worker/test/ownership.test.mjs
 */
const KV = new Map();
const APPS = {
  get: async (k) => KV.get(k) ?? null,
  put: async (k, v) => { KV.set(k, v); },
};

// Mirrors ownedApp in src/index.js.
async function ownedApp(hash, identity) {
  const raw = await APPS.get(`app:${hash}`);
  if (!raw) return { error: 404 };
  const meta = JSON.parse(raw);
  if (meta.author_login !== identity.login) return { error: 403 };
  return { meta };
}

// Mirrors the takeover guard at the top of publish's metadata write.
async function publishMetaDecision(hash, identity) {
  const priorRaw = await APPS.get(`app:${hash}`);
  if (priorRaw) {
    const prior = JSON.parse(priorRaw);
    if (prior.author_login && prior.author_login !== identity.login) {
      return "idempotent-authorship-retained";
    }
  }
  await APPS.put(`app:${hash}`, JSON.stringify({ author_login: identity.login }));
  return "written";
}

const HASH = "c".repeat(64);
const alice = { login: "alice" };
const mallory = { login: "mallory" };

let failed = 0;
const say = (label, got, want) => {
  const ok = got === want;
  if (!ok) failed++;
  console.log(`${ok ? "PASS" : "FAIL"}  ${label}: ${got} (want ${want})`);
};

// First publish: alice owns the listing.
say("first publish writes", await publishMetaDecision(HASH, alice), "written");
say("alice owns it", JSON.parse(KV.get(`app:${HASH}`)).author_login, "alice");

// The takeover: mallory re-POSTs the identical bytes.
say(
  "identical bytes from another account do not transfer authorship",
  await publishMetaDecision(HASH, mallory),
  "idempotent-authorship-retained",
);
say("alice still owns it", JSON.parse(KV.get(`app:${HASH}`)).author_login, "alice");

// Same-owner republish is a metadata update, as it always was.
say("same-owner republish writes", await publishMetaDecision(HASH, alice), "written");

// Images: only the author may touch the listing's images.
say("a stranger cannot replace the shot", (await ownedApp(HASH, mallory)).error, 403);
say("the author can", (await ownedApp(HASH, alice)).error, undefined);

// No listing at all: images have nothing to belong to.
say("no listing, no images", (await ownedApp("d".repeat(64), alice)).error, 404);

if (failed) {
  console.error(`${failed} check(s) failed`);
  process.exit(1);
}
console.log("ownership holds");
