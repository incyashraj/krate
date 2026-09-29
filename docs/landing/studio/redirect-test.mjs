// The two redirect decisions, run as the shipped code writes them.
import { readFileSync } from "node:fs";

const src = readFileSync("docs/landing/studio/bridge.js", "utf8");
const fn = src.slice(src.indexOf("function requireSignIn()"));
const body = fn.slice(0, fn.indexOf("\n}\n") + 3);

function studioDecision({ token, search }) {
  let went = null;
  const bridge = { token };
  const location = { replace: (u) => { went = u; }, search };
  const URLSearchParamsLocal = URLSearchParams;
  eval(`${body.replace("location.search", "location.search")}\nrequireSignIn();`);
  return went;
}
// A tiny harness that mirrors /login's guard, read from the page itself.
const login = readFileSync("docs/landing/login/index.html", "utf8");
const hasGuard = login.includes('if (next === "studio" && !fromApp)') && login.includes('location.replace("/app/")');

const cases = [
  ["signed out, plain visit", { token: null, search: "" }, "/login/?next=studio"],
  ["signed in", { token: "krate_tok", search: "" }, null],
  // There is no look-around door: a signed-out visit goes to sign in, whatever the query.
  ["signed out with a stray ?stay", { token: null, search: "?stay" }, "/login/?next=studio"],
];
let bad = 0;
for (const [name, input, want] of cases) {
  const got = studioDecision(input);
  const ok = got === want;
  if (!ok) bad++;
  console.log(`${ok ? "ok  " : "FAIL"} ${name}: -> ${got}`);
}
console.log(`${hasGuard ? "ok  " : "FAIL"} /login sends a signed-in visitor straight to /app`);
if (!hasGuard) bad++;

// A session that dies while the tab is open (K-793). The build service's
// 401 means the session is over; Plan and Make must both go to sign in
// once, forget the dead token and keep the sentence -- not show a bare
// "Sign in first.", and not, from Plan, promise a build first.
{
  const grab = (name) => {
    const at = src.indexOf(`  async ${name}(`);
    return src.slice(at, src.indexOf("\n  },\n", at)) + "\n  }";
  };
  const helper = src.slice(src.indexOf("function sessionOver("));
  const sessionOverSrc = helper.slice(0, helper.indexOf("\n}\n") + 3);
  // The session clock the bridge writes with (K-938), lifted like the rest.
  const nowAt = src.indexOf("function nowSecs()");
  const nowSecsSrc = src.slice(nowAt, src.indexOf("}", nowAt) + 1);
  for (const name of ["plan_request", "create_app"]) {
    const bridge = { token: "krs_expired" };
    const went = [];
    const removed = [];
    const dropped = [];
    const localStorage = { removeItem: (k) => removed.push(k) };
    const TOKEN_KEY = "krate_tok";
    const goSignIn = (r) => { went.push(r); return new Promise(() => {}); };
    const dropUnbuilt = (r) => dropped.push(r);
    const builder = async () => { const e = new Error("Sign in first."); e.status = 401; throw e; };
    const COMMANDS = {};
    const run = eval(`(function () {
      ${sessionOverSrc}
      ${nowSecsSrc}
      const webMode = () => "build";
      const tooLong = () => null;
      const deviceId = () => "dev";
      const attachmentsFor = () => [];
      const localSessions = () => [];
      COMMANDS.session_save = async () => {};
      return ({\n${grab(name)}\n});
    })()`);
    let settled = false;
    run[name]({ request: "a tiny timer" }).then(() => { settled = true; }, () => { settled = true; });
    await new Promise((r) => setTimeout(r, 10));
    const ok = went.length === 1 && went[0] === "a tiny timer" && bridge.token === null
      && removed.includes("krate_tok") && !settled && dropped.includes("a tiny timer");
    if (!ok) bad++;
    console.log(`${ok ? "ok  " : "FAIL"} ${name}: an expired session goes to sign in once, request kept -> ${JSON.stringify({ went, token: bridge.token, settled })}`);
  }
}
process.exit(bad ? 1 : 0);
