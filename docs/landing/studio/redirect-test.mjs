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
  ["signed out but ?stay", { token: null, search: "?stay" }, null],
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
process.exit(bad ? 1 : 0);
