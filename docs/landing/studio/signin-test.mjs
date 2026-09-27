// A sign-in link from someone else must not sign this browser in (K-909).
//
// Runs the SHIPPED scripts -- /login/signin.js, the /login page's own
// script and /login/done's -- in a small fake browser, and walks the attack:
// a link to /login/done/#token=<the attacker's session> used to be stored
// as this browser's session, and /make?token=... likewise. The positive
// control matters as much: the same harness, with a sign-in this browser
// really started, DOES store the session -- so a refusal below is the
// nonce at work, not a harness that cannot store anything.
//
//   node docs/landing/studio/signin-test.mjs
import { readFileSync } from "node:fs";
import vm from "node:vm";
import { webcrypto } from "node:crypto";

const signinJs = readFileSync("docs/landing/login/signin.js", "utf8");
const loginHtml = readFileSync("docs/landing/login/index.html", "utf8");
const doneHtml = readFileSync("docs/landing/login/done/index.html", "utf8");
const makeJs = readFileSync("docs/landing/make/make.js", "utf8");

/// The last inline <script> in a page: its behaviour, not its markup.
function inlineScript(html) {
  const blocks = [...html.matchAll(/<script>([\s\S]*?)<\/script>/g)].map((m) => m[1]);
  return blocks[blocks.length - 1];
}

function storage(seed = {}) {
  const m = new Map(Object.entries(seed));
  return {
    getItem: (k) => (m.has(k) ? m.get(k) : null),
    setItem: (k, v) => { m.set(k, String(v)); },
    removeItem: (k) => { m.delete(k); },
    _map: m,
  };
}

function element() {
  const classes = new Set(["hidden"]);
  const on = {};
  return {
    textContent: "", href: "", disabled: false,
    classList: {
      add: (c) => classes.add(c), remove: (c) => classes.delete(c),
      contains: (c) => classes.has(c),
    },
    addEventListener(type, fn) { (on[type] ||= []).push(fn); },
    click() { for (const fn of on.click || []) fn({ preventDefault() {} }); },
    get shown() { return !classes.has("hidden"); },
  };
}

/// One browser: its storage persists across the pages it visits.
function browser() {
  const localStorage = storage();
  function visit(url, html) {
    const u = new URL(url);
    const els = {};
    const location = {
      search: u.search, hash: u.hash, pathname: u.pathname,
      href: u.href, replace(to) { this.href = to; },
    };
    const ctx = {
      localStorage, location, URLSearchParams, Date, JSON,
      crypto: webcrypto, Uint8Array,
      history: { replaceState(_s, _t, to) { location.hash = ""; location.cleared = to; } },
      document: {
        title: "",
        documentElement: { className: "", classList: { contains: () => false } },
        getElementById: (id) => (els[id] ||= element()),
        body: { insertAdjacentHTML() {} },
      },
      fetch: () => new Promise(() => {}),
    };
    ctx.window = ctx;
    vm.createContext(ctx);
    vm.runInContext(signinJs, ctx);
    vm.runInContext(inlineScript(html), ctx);
    return { ctx, els, location };
  }
  return { localStorage, visit };
}

let bad = 0;
function check(ok, name) {
  if (!ok) bad++;
  console.log(`${ok ? "ok  " : "FAIL"} ${name}`);
}

const ATTACKER = "krs_attackers_own_session";
const fragment = (fields) => "#" + new URLSearchParams(fields).toString();

/* ---- the attack: a finished sign-in from somewhere else ----------------- */
{
  const b = browser();
  const page = b.visit(
    "https://krate.tech/login/done/" + fragment({ token: ATTACKER, login: "mallory", nonce: "f".repeat(32) }),
    doneHtml,
  );
  check(b.localStorage.getItem("krate_tok") === null, "a forged link with a made-up nonce does not sign this browser in");
  check(/did not start here/i.test((page.els.doneH || {}).textContent || ""), "and the page says why, in words");
  check(page.location.hash === "", "and the token is out of the address bar");
}
{
  const b = browser();
  b.visit("https://krate.tech/login/done/" + fragment({ token: ATTACKER, login: "mallory" }), doneHtml);
  check(b.localStorage.getItem("krate_tok") === null, "a forged link with no nonce does not sign this browser in");
}
{
  // The desktop version of the same trick: no hop to krate:// either.
  const b = browser();
  const page = b.visit("https://krate.tech/login/done/?app=1" + fragment({ token: ATTACKER, login: "mallory" }), doneHtml);
  check(!String(page.location.href).startsWith("krate://"), "a forged desktop hand-off does not reach the desktop");
}

/* ---- the control: a sign-in this browser started ------------------------ */
{
  const b = browser();
  const login = b.visit("https://krate.tech/login/?next=make", loginHtml);
  const gh = new URL(login.els.ghBtn.href);
  const nonce = gh.searchParams.get("nonce");
  check(/^[0-9a-f]{32}$/.test(nonce || ""), "the GitHub door carries a nonce");
  check(new URL(login.els.googleBtn.href).searchParams.get("nonce") === nonce, "and so does the Google door");
  // ...the provider round trip; the hub hands the nonce back...
  const done = fragment({ token: "krs_mine", login: "me", nonce });
  b.visit("https://krate.tech/login/done/" + done, doneHtml);
  check(b.localStorage.getItem("krate_tok") === "krs_mine", "a sign-in this browser started IS kept (control)");
  // Replaying the same finished link is refused: the nonce was used up.
  b.localStorage.removeItem("krate_tok");
  b.visit("https://krate.tech/login/done/" + done, doneHtml);
  check(b.localStorage.getItem("krate_tok") === null, "the same link cannot be used twice");
}
{
  // From the desktop: the desktop's own nonce goes out and comes back.
  const b = browser();
  // The new Studio sends ONLY app_nonce (a second `&` is cut off by
  // Windows' `start`), so the page must read that alone as "from the app".
  const login = b.visit("https://krate.tech/login?app_nonce=" + "a".repeat(32), loginHtml);
  const gh = new URL(login.els.ghBtn.href);
  check(gh.searchParams.get("from") === "app" && gh.searchParams.get("app_nonce") === "a".repeat(32),
    "a desktop sign-in passes the desktop's nonce to the hub");
  // A released Studio sends ?from=app with no nonce of its own: still a
  // desktop sign-in, so it keeps working until people update.
  const old = new URL(browser().visit("https://krate.tech/login?from=app", loginHtml).els.ghBtn.href);
  check(old.searchParams.get("from") === "app" && !old.searchParams.has("app_nonce"),
    "a released desktop's sign-in still goes to the desktop");
  const page = b.visit(
    "https://krate.tech/login/done/?app=1" +
      fragment({ token: "krs_mine", login: "me", nonce: gh.searchParams.get("nonce"), app_nonce: "a".repeat(32) }),
    doneHtml,
  );
  const hop = new URL(page.location.href);
  check(hop.protocol === "krate:" && hop.searchParams.get("token") === "krs_mine", "and the desktop hop happens (control)");
  check(hop.searchParams.get("app_nonce") === "a".repeat(32), "carrying the desktop's nonce for its own check");
  check(b.localStorage.getItem("krate_tok") === null, "a desktop sign-in is not stored as the website's session");
}

/* ---- /login has a way out (K-859) --------------------------------------- */
{
  const where = (search) => (browser().visit("https://krate.tech/login/" + search, loginHtml).els.notNow || {}).href;
  check(where("?next=studio") === "/app/?stay", "Not now from the Studio goes back to it, without bouncing here again");
  check(where("?next=make") === "/make/", "Not now from /make goes back to /make");
  check(where("?next=publish") === "/publish/", "Not now from publishing goes back there");
  check(where("") === "/", "Not now from nowhere goes home");
  const b = browser();
  const page = b.visit("https://krate.tech/login/?next=studio", loginHtml);
  check(b.localStorage.getItem("krate_next") === "studio", "the continuation is kept while they decide");
  if (page.els.notNow) page.els.notNow.click();
  check(b.localStorage.getItem("krate_next") === null, "and dropped when they say Not now");
  check(/<a href="\/"[^>]*><img src="\/krate-logo\.png"/.test(loginHtml), "the logo is a link home");
}

/* ---- /make takes no token from its URL ---------------------------------- */
check(!/\.get\(\s*["']token["']\s*\)/.test(makeJs), "/make has no ?token= door");
check(/signIn\(path\)[\s\S]{0,200}nonce=/.test(makeJs), "/make's sign-in sheet sends a nonce");

process.exit(bad ? 1 : 0);
