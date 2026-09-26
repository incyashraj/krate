/* What a person is told when a build fails.
 *
 * This is the first thing a stranger will meet on a bad day, and every
 * wrong answer here costs more than the failure itself: a vendor outage
 * read as "this product does not work", or worse, as "you asked for
 * something we cannot do" -- blaming somebody for our own broken key.
 *
 * Driven against the strings the ENGINE actually emits. They are quoted
 * from crates/cli/src/api_author.rs rather than invented, because the
 * whole defect this guards against was a classifier matched to imagined
 * error text: only the 429 branch matched reality, so on the day the API
 * key goes on, a rejected key, an exhausted credit balance, a retired
 * model and an Anthropic outage would all have shown the shrug.
 *
 * plainFailure is not exported -- the builder is one file with no module
 * boundary -- so it is lifted out of the source and run. A copy that
 * drifts fails loudly, which is the behaviour worth having.
 *
 *   node cloud/builder/test/failure-words.test.mjs
 */
import { readFileSync } from "node:fs";
import assert from "node:assert/strict";

const src = readFileSync("cloud/builder/src/server.js", "utf8");

const m = /function plainFailure\(tail\)\s*\{/.exec(src);
assert.ok(m, "plainFailure exists in the builder");
let depth = 0;
let body = "";
for (let i = m.index + m[0].length - 1; i < src.length; i++) {
  if (src[i] === "{") depth++;
  else if (src[i] === "}" && --depth === 0) {
    body = src.slice(m.index, i + 1);
    break;
  }
}
assert.ok(body.endsWith("}"), "plainFailure has a closing brace");
const plainFailure = new Function(`${body}; return plainFailure;`)();

const SHRUG = "That one didn't come together. Your words are still here.";
const ON_US = /on us/i;
const OUR_SIDE = /problem on our side/i;
const BLAME = /asks for something Krate cannot do/i;

// The engine's real words. Quoted from api_author.rs -- if these drift,
// this test is the thing that should notice.
const ENGINE = {
  "bad key (401/403)":
    "Anthropic rejected the API key. Check it in Settings, or set ANTHROPIC_API_KEY.",
  "rate limited (429)": "Anthropic is rate limiting this key right now.",
  "retired model (404)":
    "Anthropic does not serve the model `claude-opus-5` (it may have been retired). Set KRATE_ANTHROPIC_MODEL to a current model and try again, or update Krate for a newer default.",
  "overloaded (529)": "Anthropic returned 529: {\"type\":\"overloaded_error\"}",
  "server error (500)": "Anthropic returned 500: internal server error",
  "no credit (400)":
    "Anthropic returned 400: your credit balance is too low to access the API",
  "network unreachable": "could not reach Anthropic: connection refused",
  "model gave up": "the model stopped before the app passed check-app",
  "rounds exhausted": "the app did not pass check-app within 40 rounds",
};

// ---- nothing that is our fault may read as a shrug -----------------------
for (const [label, tail] of Object.entries(ENGINE)) {
  const said = plainFailure(tail);
  assert.notStrictEqual(
    said,
    SHRUG,
    `${label}: a failure we caused must not read as "it just didn't work" -- got the shrug for: ${tail}`,
  );
  assert.ok(
    !BLAME.test(said),
    `${label}: OUR failure must never be reported as the person asking for too much -- got: ${said}`,
  );
}
console.log("ok  every engine failure is owned, and none blames the person");

// ---- a dead key does not send somebody round a retry loop ----------------
{
  const key = plainFailure(ENGINE["bad key (401/403)"]);
  assert.ok(
    OUR_SIDE.test(key),
    `a rejected key is ours to fix and must say so: ${key}`,
  );
  assert.ok(
    !/try again in a minute/i.test(key),
    `and must NOT tell somebody to retry -- nothing they do can fix our key: ${key}`,
  );
  const credit = plainFailure(ENGINE["no credit (400)"]);
  assert.ok(OUR_SIDE.test(credit), `an empty balance is ours too: ${credit}`);
  console.log("ok  a dead key or an empty balance does not promise a retry that cannot work");
}

// ---- a transient outage DOES invite a retry ------------------------------
{
  for (const label of ["rate limited (429)", "overloaded (529)", "network unreachable"]) {
    const said = plainFailure(ENGINE[label]);
    assert.ok(ON_US.test(said), `${label} is transient and ours: ${said}`);
  }
  console.log("ok  a transient outage says it is ours and worth trying again");
}

// ---- the person's own key must never be leaked into the page -------------
{
  for (const tail of Object.values(ENGINE)) {
    const said = plainFailure(tail);
    assert.ok(
      !/ANTHROPIC_API_KEY|KRATE_ANTHROPIC_MODEL|sk-/.test(said),
      `an env var name or key fragment reached a stranger's page: ${said}`,
    );
    assert.ok(
      !/Settings|api_author|\.rs:/i.test(said),
      `internal wording reached a stranger's page: ${said}`,
    );
  }
  console.log("ok  no env var, key or internal path reaches the page");
}

// ---- a real refusal still reads as one -----------------------------------
{
  const refusal = plainFailure("krate cannot build that: it asks to read the whole disk");
  assert.ok(
    BLAME.test(refusal),
    `the wall refusing a request is not a failure and must say so: ${refusal}`,
  );
  // And the branch must not be so broad that a vendor body wins it. This
  // exact shape -- an API error containing "will not" -- used to be
  // reported as the person's fault.
  const outage = plainFailure(
    'Anthropic returned 500: {"error":"the service will not respond"}',
  );
  assert.ok(
    !BLAME.test(outage),
    `an outage body containing "will not" must not become the person's fault: ${outage}`,
  );
  // And the same body WITHOUT a status code in it. The status-code form is
  // claimed by the vendor branches before the refusal branch ever sees it,
  // so widening that branch back to "will not" is invisible unless the
  // string is one nothing else claims -- which is exactly the shape a
  // wrapped or re-thrown vendor error takes.
  for (const bare of [
    "the upstream will not respond",
    "the provider cannot do that right now",
  ]) {
    assert.ok(
      !BLAME.test(plainFailure(bare)),
      `a bare vendor message must not become the person's fault: ${bare} -> ${plainFailure(bare)}`,
    );
  }
  console.log("ok  a genuine refusal reads as one, and an outage cannot impersonate it");
}

// ---- the budget ceiling stays money-free ---------------------------------
{
  const said = plainFailure("that is more work than we allow in one build");
  assert.match(said, /bigger than a single build/i, `the ceiling has its own words: ${said}`);
  assert.ok(
    !/\$|cost|budget|money|spend/i.test(said),
    `our bill is not the person's problem and must not appear: ${said}`,
  );
  console.log("ok  the spend ceiling never mentions money to the person");
}

console.log("ok  a failed build tells the truth about whose fault it was");
