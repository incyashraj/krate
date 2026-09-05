/* A stand-in for the hub, so the whole make-in-browser experience can be
 * walked, judged and fixed before a single cent of inference is spent.
 *
 * It answers the SAME contract the real service will: POST /build returns
 * a job id, GET /build/<id> reports stage, line, shot and result. That is
 * the point -- when the real builder exists, this file is deleted and
 * nothing in make.js changes.
 *
 * Loaded only when the page is opened with ?mock=1, so it can never
 * intercept anything in production.
 */

const REAL_FETCH = window.fetch.bind(window);
const HUB = "https://hub.krate.tech";

/* A real screenshot of a real Krate app, so the frame fills with something
 * honest rather than a grey rectangle pretending to be a preview. */
const SHOT = "/make/rate-card.png";

const jobs = new Map();

// A signed-in session, so the mock walks the path that matters. Without
// this the page correctly shows its signed-out face and the account,
// shelf and wall never get exercised.
try {
  if (!localStorage.getItem("krate-token")) {
    localStorage.setItem("krate-token", "krs_mock_session");
  }
} catch (e) {}

/* The pace a real build actually moves at, compressed. A mock that
 * finishes instantly teaches nothing about whether the wait is bearable,
 * which is the main thing this page has to get right. */
const SCRIPT = [
  { at: 400,   stage: "read",  line: "reading what Krate can do" },
  { at: 3200,  stage: "write", line: "writing the code" },
  { at: 7000,  stage: "write", line: "laying out the screen" },
  { at: 10500, stage: "test",  line: "opening your app", shot: true },
  { at: 14000, stage: "test",  line: "clicking through it", shot: true },
  { at: 17000, stage: "done",  line: "packing it into one file", shot: true },
  { at: 20000, done: true },
];

window.fetch = async function (input, init = {}) {
  const url = typeof input === "string" ? input : input.url;
  if (!url.startsWith(HUB)) return REAL_FETCH(input, init);
  const path = url.slice(HUB.length).split("?")[0];
  const method = (init.method || "GET").toUpperCase();

  // Signed in, on the free plan, with two of three makes already used --
  // the state most worth designing against, since it is one make from the
  // wall.
  if (path === "/me") {
    return reply({
      user: { name: "Yashraj Pardeshi", login: "incyashraj", email: "you@example.com", avatar_url: "" },
      plan: { plan: "free", active: false, until: 0, via: "", portal: false },
      referral: { code: "abc123", count: 0, awards: 0 },
    });
  }

  if (path === "/my/apps") {
    return reply({ apps: [
      { url: "#", meta: { name: "Rate card", size: 85210 } },
      { url: "#", meta: { name: "Trip splitter", size: 91044 } },
    ]});
  }

  if (path === "/build" && method === "POST") {
    // Flip this to walk the wall instead of a build.
    if (new URLSearchParams(location.search).get("wall") === "1") {
      // One funded first working app per account (Master Plan CP0; closes
      // the policy half of K-216). After it, Studio with their own AI.
      return reply("Your first app was on us. Keep making in Studio with your own AI.", 402);
    }
    const id = "job_" + Math.random().toString(36).slice(2, 9);
    jobs.set(id, { started: Date.now() });
    return reply({ id });
  }

  if (path.startsWith("/build/") && path.endsWith("/stop")) {
    jobs.delete(path.split("/")[2]);
    return reply({ ok: true });
  }

  if (path.startsWith("/build/")) {
    const job = jobs.get(path.split("/")[2]);
    if (!job) return reply("no such build", 404);
    const age = Date.now() - job.started;
    const step = [...SCRIPT].reverse().find((s) => age >= s.at) || SCRIPT[0];
    if (step.done) {
      return reply({
        state: "done",
        result: {
          id: "app_demo",
          name: "Rate card",
          size: "85 KB",
          asks: ["open a window"],
          shot: SHOT,
          download: "/evidence/demo/ratecard.krate",
        },
      });
    }
    return reply({
      state: "working",
      stage: step.stage,
      line: step.line,
      shot: step.shot ? SHOT : null,
    });
  }

  return REAL_FETCH(input, init);
};

function reply(body, status = 200) {
  const isText = typeof body === "string";
  return new Response(isText ? body : JSON.stringify(body), {
    status,
    headers: { "content-type": isText ? "text/plain" : "application/json" },
  });
}

console.info("krate: mock hub active (?mock=1). Real network calls are untouched.");
