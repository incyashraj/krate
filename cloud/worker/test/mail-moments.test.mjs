/* The mail a person gets about their own things (cloud/worker/src/mail.js):
 * their app is live, someone opened it for the first time, the app they
 * asked for is made. Each moment mails once, and none of them can fail
 * what it is about. Resend is a stub: nothing here sends real mail.
 *
 *   node --experimental-wasm-modules --test cloud/worker/test/mail-moments.test.mjs
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { orgEnv, call, tokenFor, worker } from "./org-tools.mjs";
import { sha256Hex } from "./r2-mock.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const ported = (name) => new Uint8Array(readFileSync(join(here, "..", "..", "..", "evidence", "ported", name)));

/* Resend, standing in: every mail it is handed, and an optional refusal. */
function stubResend() {
  const sent = [];
  const real = globalThis.fetch;
  const stub = { sent, refuse: false };
  globalThis.fetch = async (input, init = {}) => {
    if (String(input.url || input) === "https://api.resend.com/emails") {
      if (stub.refuse) return new Response("refused", { status: 422 });
      sent.push(JSON.parse(init.body));
      return new Response(JSON.stringify({ id: "sent" }), { headers: { "content-type": "application/json" } });
    }
    return real(input, init);
  };
  stub.restore = () => {
    globalThis.fetch = real;
  };
  return stub;
}

const hubEnv = () => {
  const env = orgEnv({ RESEND_API_KEY: "re_test", VISITOR_SALT: "salt", PUBLIC_BASE: "https://hub.example" });
  env.USAGE = { writeDataPoint() {} };
  return env;
};

test("a publish mails its publisher once, with the link; a quiet publish or the same bytes again mails nobody", async () => {
  const mail = stubResend();
  try {
    const env = hubEnv();
    const up = await call(env, "ada@example.com", "POST", "/publish", { bytes: ported("bounce.krate"), headers: { "x-krate-name": "Bounce" } });
    assert.equal(up.status, 200, JSON.stringify(up.body));
    assert.equal(mail.sent.length, 1);
    const [m] = mail.sent;
    assert.deepEqual(m.to, ["ada@example.com"]);
    assert.equal(m.from, "Krate <hello@krate.tech>");
    assert.equal(m.reply_to, "incyashraj@gmail.com");
    assert.equal(m.subject, "Bounce is live on Krate");
    assert.ok(m.text.includes(up.body.url), m.text);
    assert.ok(m.text.includes("opens in any browser, on a phone or a computer"), m.text);
    // The same bytes again: the hub already holds them, so nothing new is live.
    await call(env, "ada@example.com", "POST", "/publish", { bytes: ported("bounce.krate"), headers: { "x-krate-name": "Bounce" } });
    assert.equal(mail.sent.length, 1, "the same app is not announced twice");
    // Studio's Open it here publishes quietly: no mail for opening a tab.
    const quiet = await call(env, "ada@example.com", "POST", "/publish", { bytes: ported("chart.krate"), headers: { "x-krate-unlisted": "1", "x-krate-quiet": "1" } });
    assert.equal(quiet.status, 200, JSON.stringify(quiet.body));
    assert.equal(mail.sent.length, 1, "a quiet publish mails nobody");
    // Resend refusing is not a failed publish.
    mail.refuse = true;
    const refused = await call(env, "bob@example.com", "POST", "/publish", { bytes: ported("chart.krate"), headers: { "x-krate-name": "Chart" } });
    assert.ok(refused.status === 200 || refused.status === 409, `a refused mail never fails the publish: ${refused.status}`);
  } finally {
    mail.restore();
  }
});

test("the first time someone else opens an app in a tab, its publisher hears once; the maker's own opens never mail", async () => {
  const mail = stubResend();
  try {
    const env = hubEnv();
    const up = await call(env, "ada@example.com", "POST", "/publish", { bytes: ported("bounce.krate"), headers: { "x-krate-name": "Bounce" } });
    assert.equal(up.status, 200, JSON.stringify(up.body));
    const hash = await sha256Hex(ported("bounce.krate"));
    mail.sent.length = 0;
    const open = (who) =>
      worker.fetch(
        new Request("https://hub.example/tab", {
          method: "POST",
          headers: {
            "content-type": "application/json",
            "user-agent": "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1",
            ...(who ? { authorization: `Bearer ${tokenFor(env, who)}` } : {}),
          },
          body: JSON.stringify({ event: "open", app: hash }),
        }),
        env,
      );
    await open("ada@example.com");
    assert.equal(mail.sent.length, 0, "the maker opening their own app is not news");
    await open(null);
    assert.equal(mail.sent.length, 1);
    assert.equal(mail.sent[0].subject, "Someone just opened Bounce");
    assert.deepEqual(mail.sent[0].to, ["ada@example.com"]);
    assert.ok(mail.sent[0].text.includes(`https://hub.example/a/${hash}`), mail.sent[0].text);
    await open(null);
    await open("bob@example.com");
    assert.equal(mail.sent.length, 1, "only the first open by someone else");
  } finally {
    mail.restore();
  }
});

test("the first file a build makes is mailed to the person who asked for it, once", async () => {
  const mail = stubResend();
  try {
    const env = hubEnv();
    const device = "d".repeat(64);
    const post = (path, payload) =>
      worker.fetch(
        new Request(`https://hub.example${path}`, {
          method: "POST",
          headers: { "content-type": "application/json", authorization: `Bearer ${tokenFor(env, "ada@example.com")}` },
          body: JSON.stringify(payload),
        }),
        env,
      );
    const opened = await (await post("/case/open", { device, request: "a habit tracker with streaks" })).json();
    assert.ok(opened.id, JSON.stringify(opened));
    await post("/case/attempt", { device, id: opened.id, outcome: "failed" });
    assert.equal(mail.sent.length, 0, "a failed try is not mailed");
    await post("/case/attempt", { device, id: opened.id, outcome: "made" });
    assert.equal(mail.sent.length, 1);
    assert.equal(mail.sent[0].subject, "Your app is ready");
    assert.deepEqual(mail.sent[0].to, ["ada@example.com"]);
    assert.ok(mail.sent[0].text.includes("a habit tracker with streaks"), mail.sent[0].text);
    assert.ok(mail.sent[0].text.includes("https://krate.tech/app/"), mail.sent[0].text);
    await post("/case/attempt", { device, id: opened.id, outcome: "made" });
    assert.equal(mail.sent.length, 1, "a change to the same app is not a new app");
  } finally {
    mail.restore();
  }
});
