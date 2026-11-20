// Make an app from any AI chat, with no Krate tool in it: GET /build.
//
// Most chats cannot call Krate (free ChatGPT, grok.com, Gemini outside the
// US, Meta AI, DeepSeek...), but every one of them can write text. Krate's
// guide (krate-mode.md, "If you have no Krate tool") teaches the AI to put
// every file of the app in one code block, each file opened by a line
//
//     --- FILE: src/lib.rs ---
//
// and the block closed by `--- END ---`. The person copies that answer,
// pastes it here, signs in with the hub's own sign-in, and the page sends
// the files down the very path the connector's build_app takes (mcp.js,
// callToolAs): the same builder, the same per-account limits (10 an hour,
// 30 a day, counted together with the connector's), the same check words,
// the same unlisted publish and the same /run/ link. A failed check comes
// back as a box of text to paste into the chat, so the AI fixes its own
// code. No AI of Krate's runs here, and nothing here can spend a key.
//
//   GET  /build                the page
//   POST /build/start          { text }  (hub session)  -> 202 { build_id, name, files }
//                                                       | 4xx { problem, copy_back }
//   GET  /build/status/<id>    (hub session)            -> { status, ...build_status's fields, copy_back? }
//
// The parser runs twice, the same function both times: in the page, to show
// what it found as the person pastes, and here, which is the one that counts.

import { callToolAs } from "./mcp.js";

/** The most a paste may be, in characters: the build's 2 MB of source and room for the AI's words around it. */
export const MAX_PASTE = 3 * 1024 * 1024;

/**
 * Read an AI's answer into an app's files. Self-contained on purpose: the
 * page carries this function's own source, so it may use nothing from
 * outside its body.
 *
 * Answers { name, files: [{ path, content }] } or { problem }.
 */
export function parsePastedApp(input) {
  const MAX_CHARS = 3 * 1024 * 1024;
  const MAX_SOURCE = 2 * 1024 * 1024;
  const MAX_FILES = 64;
  const NEEDED = ["Cargo.toml", "manifest.toml", "src/lib.rs"];
  // "--- FILE: src/lib.rs ---", and the forms an AI drifts into: ===, ###,
  // **bold**, a path in backticks, "File:" in any case.
  const MARK = /^[ \t>]*(?:[-=#*_]+[ \t]*)?FILE[ \t]*:[ \t]*[`'"*]*([^`'"*\s][^`'"*]*?)[`'"*]*[ \t]*(?:[-=#*_]+)?[ \t]*$/i;
  const END = /^[ \t>]*(?:[-=#*_]+[ \t]*)?END(?:[ \t]+OF[ \t]+FILES)?[ \t]*(?:[-=#*_]+)?[ \t]*$/i;
  const OPEN = /^[ \t]*(`{3,}|~{3,})[ \t]*[\w+.#-]*[ \t]*$/;
  const CLOSE = /^[ \t]*(`{3,}|~{3,})[ \t]*$/;
  const ANY_FENCE = /^[ \t]*(`{3,}|~{3,})/;
  // What a chat writes when it labels each file's own code block, as the
  // guide's worked examples do: "`manifest.toml`:", "**src/lib.rs**",
  // "### Cargo.toml". Taken as a file only outside any code block and only
  // when a code block opens on the next line that is not blank.
  const LABEL = /^[ \t>]*(?:#{1,6}[ \t]+)?[*_`'"]*((?:[\w.-]+\/)*[\w.-]+\.(?:toml|rs|md|txt|json|csv|lock))[*_`'"]*[ \t]*:?[ \t]*[*_]*[ \t]*$/;

  if (typeof input !== "string" || !input.trim()) return { problem: "Nothing was pasted. Paste your AI's whole answer." };
  if (input.length > MAX_CHARS) return { problem: "That is more than 3 MB of text. An app's source is far smaller: paste only the AI's answer with the files." };
  const lines = input.replace(/\r\n?/g, "\n").split("\n");

  const found = [];
  let current = null;
  let ended = false;
  let fence = null;
  for (let n = 0; n < lines.length; n += 1) {
    const line = lines[n];
    if (ended) break;
    const mark = MARK.exec(line);
    if (mark) {
      current = { path: mark[1].trim(), lines: [] };
      found.push(current);
      continue;
    }
    if (!fence) {
      const label = LABEL.exec(line);
      if (label) {
        let next = n + 1;
        while (next < lines.length && !lines[next].trim()) next += 1;
        if (next < lines.length && OPEN.test(lines[next])) {
          current = { path: label[1], lines: [] };
          found.push(current);
          continue;
        }
      }
      const opened = OPEN.exec(line);
      if (opened) fence = opened[1];
    } else {
      const closed = CLOSE.exec(line);
      if (closed && closed[1][0] === fence[0] && closed[1].length >= fence.length) fence = null;
    }
    if (END.test(line) && current) {
      ended = true;
      continue;
    }
    if (current) current.lines.push(line);
  }
  if (!found.length) {
    return {
      problem:
        "No files were found. Each file has to start with a line like --- FILE: src/lib.rs --- and the last one end with --- END ---.",
    };
  }

  const files = [];
  const seen = new Set();
  let total = 0;
  for (let i = 0; i < found.length; i += 1) {
    const f = found[i];
    const path = f.path.replace(/^\.\//, "");
    if (!path || path.length > 200 || path.startsWith("/") || path.includes("\\") || /^[a-z]+:/i.test(path) ||
        path.split("/").some((p) => p === "" || p === "." || p === "..")) {
      return { problem: `"${f.path.slice(0, 80)}" is not a path inside the app folder. Use paths like src/lib.rs.` };
    }
    if (seen.has(path)) return { problem: `${path} is in the answer twice. Each file should appear once.` };
    seen.add(path);
    let body = f.lines.slice();
    const trim = () => {
      while (body.length && !body[0].trim()) body.shift();
      while (body.length && !body[body.length - 1].trim()) body.pop();
    };
    trim();
    // A file in its own code block: take the block's inside. Words after the
    // block are the AI talking and are left out; another code block after it
    // is code that would be lost, so it is refused below.
    const open = body.length ? OPEN.exec(body[0]) : null;
    if (open) {
      const fence = open[1];
      const end = body.findIndex((l, j) => {
        if (j === 0) return false;
        const c = CLOSE.exec(l);
        return c && c[1][0] === fence[0] && c[1].length >= fence.length;
      });
      if (end > 0) {
        const after = body.slice(end + 1);
        const isLastBlock = i === found.length - 1 || ended;
        // The block around all the files may close after the last one.
        const extra = after.filter((l) => ANY_FENCE.test(l) && !(isLastBlock && CLOSE.test(l)));
        if (extra.length) {
          return {
            problem:
              `${path} is followed by another code block before the next --- FILE: line, and Krate cannot tell which ` +
              "file that code belongs to. Ask for all the files in one code block.",
          };
        }
        body = body.slice(1, end);
      } else {
        body.shift();
      }
      trim();
    }
    // The block around all the files ends after the last one.
    if (i === found.length - 1 || ended) {
      while (body.length && CLOSE.test(body[body.length - 1])) {
        body.pop();
        trim();
      }
    }
    if (!/\.(md|markdown|txt)$/i.test(path)) {
      const stray = body.findIndex((l) => ANY_FENCE.test(l));
      if (stray >= 0) {
        return {
          problem:
            `${path} has a code-block line (${body[stray].trim().slice(0, 12)}) inside it, at its line ${stray + 1}: ` +
            "the files were split into code blocks in a way Krate cannot read back. Ask for all the files in one code block.",
        };
      }
    }
    const content = body.length ? `${body.join("\n")}\n` : "";
    if (!content.trim()) return { problem: `${path} is empty.` };
    total += content.length;
    if (total > MAX_SOURCE) return { problem: "The files are more than 2 MB together. Paste the app's source, not its build output." };
    files.push({ path, content });
  }
  if (files.length > MAX_FILES) return { problem: `That is ${files.length} files; an app may have ${MAX_FILES} at most.` };
  const missing = NEEDED.filter((p) => !seen.has(p));
  if (missing.length) {
    return {
      problem: `The answer has no ${missing.join(", no ")}. A Krate app needs Cargo.toml, manifest.toml and src/lib.rs, each starting with its --- FILE: line.`,
    };
  }

  const manifest = files.find((f) => f.path === "manifest.toml").content;
  const cargo = files.find((f) => f.path === "Cargo.toml").content;
  const nameIn = (text) => {
    const m = /^[ \t]*name[ \t]*=[ \t]*"([^"\n]{1,80})"/m.exec(text);
    return m ? m[1].trim() : "";
  };
  const name = (nameIn(manifest) || nameIn(cargo).replace(/[-_]+/g, " ") || "My app").slice(0, 60);
  return { name, files };
}

/** What to paste back into the chat, so the AI fixes its own answer. */
export function copyBack(kind, words) {
  const format =
    "Send the whole app again: every file, in one code block, each file starting with a line --- FILE: <path> --- " +
    "and the block ending with --- END ---.";
  if (kind === "paste") return `Krate could not read the files: ${words}\n\n${format}`;
  return `Krate built the app and its check did not pass.\n\n${words}\n\nFix this. ${format}`;
}

/** The connector's words for its next step, as a person at this page would do it. */
const asPage = (s) => String(s || "").replace(/call build_app again/gi, "send every file again").replace(/build_app/g, "Krate");

const json = (body, status = 200) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json", "cache-control": "no-store" } });

/**
 * The /build routes, or null when the path is not one. `deps` is the MCP
 * deps (mcpDeps in index.js) plus:
 *   whoIs(request)        the signed-in hub session as the connector's grant records it, or null
 *   rateLimited(request)  a 429 to answer, or null
 */
export async function pasteBuildRoute(request, url, deps) {
  const { pathname } = url;
  if (pathname !== "/build" && pathname !== "/build/start" && !pathname.startsWith("/build/status/")) return null;
  if (request.method === "GET" && pathname === "/build") return buildPage();

  if (request.method === "POST" && pathname === "/build/start") {
    const over = await deps.rateLimited(request);
    if (over) return over;
    const who = await deps.whoIs(request);
    if (!who) return json({ problem: "Sign in to Krate first.", signed_out: true }, 401);
    // JSON's escaping makes a paste longer on the wire than in the box.
    const MAX_BODY = 4 * 1024 * 1024;
    const declared = Number(request.headers.get("content-length") || 0);
    if (declared > MAX_BODY) return json({ problem: "That is more than 3 MB of text." }, 413);
    let text = "";
    try {
      const raw = await request.text();
      if (raw.length > MAX_BODY) return json({ problem: "That is more than 3 MB of text." }, 413);
      text = JSON.parse(raw).text;
    } catch (_) {
      return json({ problem: "Send { text: the AI's answer }." }, 400);
    }
    const parsed = parsePastedApp(text);
    if (parsed.problem) return json({ problem: parsed.problem, copy_back: copyBack("paste", parsed.problem) }, 422);
    const out = await callToolAs(deps, who, "build_app", { name: parsed.name, files: parsed.files });
    if (out.error) return json({ problem: out.error.message, copy_back: copyBack("paste", out.error.message) }, 422);
    if (out.signIn) return json({ problem: "Sign in to Krate first.", signed_out: true }, 401);
    const s = out.result.structuredContent || {};
    if (out.result.isError || !s.build_id) {
      const words = asPage(s.message || out.result.content[0].text);
      // A limit reached, or a build still running, is "try later"; anything
      // else is the build service not taking it.
      return json({ problem: words }, /the most it may|still running/.test(words) ? 429 : 503);
    }
    return json({ build_id: s.build_id, name: parsed.name, files: parsed.files.map((f) => f.path) }, 202);
  }

  const status = /^\/build\/status\/([0-9a-f]{32})$/.exec(pathname);
  if (request.method === "GET" && status) {
    const who = await deps.whoIs(request);
    if (!who) return json({ problem: "Sign in to Krate first.", signed_out: true }, 401);
    const out = await callToolAs(deps, who, "build_status", { build_id: status[1] });
    if (out.error || out.signIn) return json({ problem: "That build id is not one Krate answered." }, 400);
    const s = { ...(out.result.structuredContent || {}) };
    s.message = asPage(s.message);
    if (s.fix) s.fix = asPage(s.fix);
    if (s.status === "failed") {
      const words = `Where it stopped: the ${s.stage || "check"} stage.\n\n${s.detail || ""}` + (s.fix ? `\n\nHow to fix it: ${s.fix}` : "");
      s.copy_back = copyBack("build", words);
    }
    return json(s);
  }
  return json({ problem: "not found" }, 404);
}

// --------------------------------------------------------------------- page

const PAGE_CSS = `
:root{--bg:#fafafa;--fg:#111;--muted:#5f5f66;--line:#e4e4e7;--card:#fff;--accent:#111;--on-accent:#fff;--good:#15803d;--bad:#b42318;--code:#f4f4f5}
@media (prefers-color-scheme: dark){:root{--bg:#0b0b0c;--fg:#f2f2f2;--muted:#a1a1aa;--line:#27272a;--card:#141416;--accent:#f2f2f2;--on-accent:#111;--good:#4ade80;--bad:#f87171;--code:#1c1c1f}}
*{box-sizing:border-box}html{-webkit-text-size-adjust:100%}
body{margin:0;background:var(--bg);color:var(--fg);font:15px/1.55 -apple-system,system-ui,"Segoe UI",sans-serif}
main{max-width:680px;margin:0 auto;padding:32px 16px 64px}
h1{font-size:26px;line-height:1.2;margin:0 0 8px;letter-spacing:-.02em}
h2{font-size:16px;margin:0 0 6px}
p{margin:0 0 12px;color:var(--muted)}
.step{background:var(--card);border:1px solid var(--line);border-radius:14px;padding:18px 16px;margin:0 0 14px}
.n{display:inline-flex;width:22px;height:22px;border-radius:50%;background:var(--accent);color:var(--on-accent);font-size:12px;font-weight:700;align-items:center;justify-content:center;margin-right:8px;vertical-align:1px}
textarea{width:100%;min-height:140px;padding:12px;border-radius:10px;border:1px solid var(--line);background:var(--code);color:var(--fg);font:13px/1.45 ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;resize:vertical}
pre{white-space:pre-wrap;overflow-wrap:anywhere;background:var(--code);border-radius:10px;padding:12px;margin:0 0 10px;font:13px/1.45 ui-monospace,SFMono-Regular,Menlo,Consolas,monospace}
button,.btn{display:inline-flex;align-items:center;justify-content:center;min-height:44px;padding:10px 18px;border-radius:999px;border:1px solid var(--line);background:var(--card);color:var(--fg);font:600 15px system-ui,sans-serif;cursor:pointer;text-decoration:none;margin:4px 8px 4px 0}
.primary{background:var(--accent);color:var(--on-accent);border-color:var(--accent)}
button:disabled{opacity:.5;cursor:default}
input[type=email]{width:100%;max-width:320px;min-height:44px;padding:10px 12px;border-radius:10px;border:1px solid var(--line);background:var(--bg);color:var(--fg);font:15px system-ui,sans-serif;margin:4px 8px 4px 0}
.found{color:var(--good)}.bad{color:var(--bad)}
.files{margin:8px 0 0;padding:0;list-style:none;font:13px ui-monospace,SFMono-Regular,Menlo,monospace;color:var(--muted)}
.link{display:block;overflow-wrap:anywhere;font-weight:600;color:var(--fg)}
small{color:var(--muted)}
[hidden]{display:none!important}
/* An iPhone zooms into a box whose text is under 16px when it is tapped. */
@media (max-width:600px){textarea,input[type=email]{font-size:16px}}
`;

const PROMPT =
  "Make me a Krate app: <say what the app should do>.\n\n" +
  "First read Krate's guide at https://krate.tech/krate-mode.md and follow it. " +
  "Then give me every file of the app in one code block: each file starts with a line --- FILE: <path> --- " +
  "(Cargo.toml, manifest.toml, src/lib.rs and any others), and the block ends with --- END ---.";

function pageScript() {
  return `
${parsePastedApp.toString()}
const $=(id)=>document.getElementById(id);
const KEY="krate-build-session",HELD="krate-build-paste";
const store={get(k){try{return localStorage.getItem(k)}catch(e){return null}},set(k,v){try{localStorage.setItem(k,v)}catch(e){}},del(k){try{localStorage.removeItem(k)}catch(e){}}};
// A sign-in coming back: take the session only with the nonce this browser sent.
(function(){const f=new URLSearchParams(location.hash.slice(1));if(!f.get("token"))return;history.replaceState(null,"",location.pathname);
const n=f.get("nonce")||"";if(n&&store.get("krate-build-nonce")===n){store.set(KEY,JSON.stringify({token:f.get("token"),login:f.get("login")||""}));}store.del("krate-build-nonce");})();
function session(){try{return JSON.parse(store.get(KEY)||"null")}catch(e){return null}}
function nonce(){const n=[...crypto.getRandomValues(new Uint8Array(16))].map(b=>b.toString(16).padStart(2,"0")).join("");store.set("krate-build-nonce",n);store.set(HELD,$("paste").value);return n;}
$("gh").onclick=()=>{location.href="/login/start?from=build&nonce="+nonce();};
$("gg").onclick=()=>{location.href="/login/google/start?from=build&nonce="+nonce();};
$("em").onsubmit=async(e)=>{e.preventDefault();const r=await fetch("/login/email",{method:"POST",headers:{"content-type":"application/json"},body:JSON.stringify({email:$("addr").value,from:"build",nonce:nonce()})});
$("sent").hidden=false;$("sent").textContent=r.ok?"Check your inbox and open the link on this device.":await r.text();};
$("out").onclick=()=>{store.del(KEY);show();};
function copy(id,btn){const t=$(id).value!==undefined?$(id).value:$(id).textContent;navigator.clipboard.writeText(t).then(()=>{const was=btn.textContent;btn.textContent="Copied";setTimeout(()=>btn.textContent=was,1500);});}
$("copyprompt").onclick=(e)=>copy("prompt",e.target);
$("copyback").onclick=(e)=>copy("back",e.target);
let parsed=null;
function look(){const t=$("paste").value;$("go").disabled=true;if(!t.trim()){$("seen").textContent="";$("list").textContent="";parsed=null;return;}
parsed=parsePastedApp(t);if(parsed.problem){$("seen").className="bad";$("seen").textContent=parsed.problem;$("list").textContent="";return;}
$("seen").className="found";$("seen").textContent="Found "+parsed.files.length+" files for \\u201c"+parsed.name+"\\u201d.";
$("list").textContent=parsed.files.map(f=>f.path).join("\\n");$("go").disabled=!session();}
$("paste").addEventListener("input",look);
function show(){const s=session();$("signin").hidden=!!s;$("signed").hidden=!s;if(s)$("whoami").textContent="Signed in"+(s.login?" as "+s.login:"")+".";look();}
function state(kind,words){$("status").hidden=false;$("status").className=kind==="bad"?"bad":"";$("status").textContent=words;}
async function api(path,init){const s=session();const r=await fetch(path,{...(init||{}),headers:{...((init&&init.headers)||{}),authorization:"Bearer "+(s?s.token:"")}});
const a=await r.json().catch(()=>({problem:"Krate answered something this page cannot read. Try again."}));if(r.status===401){store.del(KEY);show();}return a;}
$("go").onclick=async()=>{$("go").disabled=true;$("done").hidden=true;$("failed").hidden=true;state("","Sending the files to Krate...");
const a=await api("/build/start",{method:"POST",headers:{"content-type":"application/json"},body:JSON.stringify({text:$("paste").value})});
if(!a.build_id){state("bad",a.problem||"Krate could not start the build.");if(a.copy_back){$("back").value=a.copy_back;$("failed").hidden=false;}$("go").disabled=false;return;}
state("","Building \\u201c"+a.name+"\\u201d. Krate is compiling and checking it; this usually takes one to five minutes.");poll(a.build_id,0);};
async function poll(id,n){const a=await api("/build/status/"+id);
if(a.status==="building"){state("","Still building"+(a.stage?" ("+a.stage+")":"")+". This usually takes one to five minutes.");setTimeout(()=>poll(id,n+1),Math.min(3000+n*500,8000));return;}
$("go").disabled=false;
if(a.status==="published"){$("status").hidden=true;$("run").href=a.browser_link;$("run").textContent=a.browser_link;$("own").href=a.link;$("own").textContent=a.link;$("done").hidden=false;store.del(HELD);return;}
if(a.status==="failed"&&a.copy_back){state("bad","It did not pass Krate's check. Paste the box below into your AI chat; it will fix the code. Then paste its new answer here.");$("back").value=a.copy_back;$("failed").hidden=false;return;}
state("bad",a.message||a.problem||"Krate could not finish this build.");}
const held=store.get(HELD);if(held&&!$("paste").value)$("paste").value=held;
show();`;
}

export function buildPage() {
  const esc = (s) => String(s).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
  const body = `
<h1>Make an app in any AI chat</h1>
<p>Ask ChatGPT, Grok, Gemini, Claude or any other AI chat to write the app. Paste its answer here: Krate builds it, checks it, and gives you a link that opens on any device.</p>

<section class="step">
<h2><span class="n">1</span>Ask your AI</h2>
<p>Copy this into your chat and say what the app should do.</p>
<pre id="prompt">${esc(PROMPT)}</pre>
<button id="copyprompt">Copy</button>
</section>

<section class="step">
<h2><span class="n">2</span>Paste its answer</h2>
<p>Copy the AI's whole answer (or the one code block with the files) and paste it here.</p>
<textarea id="paste" spellcheck="false" autocomplete="off" aria-label="The AI's answer" placeholder="--- FILE: Cargo.toml ---&#10;...&#10;--- FILE: manifest.toml ---&#10;...&#10;--- FILE: src/lib.rs ---&#10;...&#10;--- END ---"></textarea>
<p id="seen" aria-live="polite"></p>
<pre class="files" id="list"></pre>
</section>

<section class="step">
<h2><span class="n">3</span>Build it</h2>
<div id="signin">
<p>Sign in so the app is yours. Free: up to 10 builds an hour and 30 a day.</p>
<button class="primary" id="gh">Continue with GitHub</button><button id="gg">Continue with Google</button>
<form id="em"><input type="email" id="addr" placeholder="you@example.com" autocomplete="email" required><button type="submit">Email me a link</button></form>
<p id="sent" hidden></p>
</div>
<div id="signed" hidden>
<p id="whoami"></p>
<button class="primary" id="go" disabled>Build my app</button><button id="out">Sign out</button>
</div>
<p id="status" hidden aria-live="polite"></p>
<div id="failed" hidden>
<h2>Copy this back to your AI</h2>
<textarea id="back" readonly aria-label="What to paste back into the chat"></textarea>
<button class="primary" id="copyback">Copy</button>
</div>
<div id="done" hidden>
<h2 class="found">Your app is ready</h2>
<p>Open it in a browser on any device, nothing to install:</p>
<a class="link" id="run" href="#"></a>
<p style="margin-top:12px">Its own link, to share. It always opens the newest version:</p>
<a class="link" id="own" href="#"></a>
<p style="margin-top:12px"><small>Published unlisted to your account: only people with the link can open it. To change it, ask your AI and paste its new answer here.</small></p>
</div>
</section>
<p><small>Krate compiles and checks the files your AI wrote. It runs no AI of its own.</small></p>`;
  return new Response(
    `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>Make an app in any AI chat -- Krate</title>
<meta name="description" content="Paste what ChatGPT, Grok, Gemini or any AI chat wrote; Krate builds it and gives you a link that opens anywhere.">
<style>${PAGE_CSS}</style></head><body><main>${body}</main><script>${pageScript()}</script></body></html>`,
    {
      headers: {
        "content-type": "text/html; charset=utf-8",
        "cache-control": "no-store",
        "referrer-policy": "no-referrer",
        "x-frame-options": "DENY",
        "content-security-policy":
          "default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline'; img-src https: data:; connect-src 'self'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'",
      },
    },
  );
}
