// In-app messages: what the founder says to the people who use Krate,
// read inside Studio (2026-11-23).
//
// A person sent a report from web Studio after two failed builds. He had
// signed in with GitHub, so the hub held no email for him, and the desk's
// only way to answer anyone was a mail. The reply had nowhere to go. Now it
// goes where the person already is: Studio, on the web and on the desktop,
// whichever way they signed in, or not signed in at all.
//
// What is kept (KV, APPS):
//
//   inbox:<principal>     {items: [...], read: ms}   one person's messages,
//                         newest first, at most ITEMS_KEPT, and how far
//                         they have read. <principal> is what the hub
//                         already knows a person by (index.js ownersOf):
//                         `account:<id>` or `github:<login>`. A person reads
//                         every principal they hold, so a message reaches
//                         them however they signed in today.
//   inbox-everyone        [{id, at, text}]           messages to everyone,
//                         stored ONCE: a broadcast is one write, never a
//                         write per person (KV's free plan is 1,000 writes
//                         a day for the whole hub).
//   inbox-sent            [...]                      what the desk sent,
//                         newest first, for its Sent list and for Remove.
//   report:<id>.thread    [{id, who, text, at}]      a report's conversation:
//                         the desk's replies ("krate") and the person's
//                         answers ("them"), on the report itself.
//   report:<id>.keys      [sha256(key)]              a report sent signed out
//                         gets a secret back (putReport answers {ok,id,key});
//                         the browser or computer that sent it reads the
//                         replies with it, no account needed.
//
// Who sees a message to everyone: every person, signed in or not, sees the
// ones sent in the last EVERYONE_DAYS days (at most EVERYONE_KEPT). Only
// ones sent after their account was made, or after they last read, count as
// unread -- so a new account can read last week's note, and is never shown
// a red dot for something sent before it existed.
//
// Person, any sign-in or none:
//   POST /inbox          {reports: [{id, key}], read}  messages, threads, unread
//   POST /inbox/read     {at}                         read up to `at`
//   POST /inbox/answer   {report, key?, text}         answer on a report thread;
//                                                     the founder is mailed
// Desk (/admin/api/, KRATE_ADMINS):
//   GET  messages                      what was sent, newest first
//   GET  messages/people?q=            find one person (login, email, name, id)
//   POST messages/send   {to, text}    to: "everyone" or an account id
//   POST messages/remove {id}          take a sent message back
//   POST report/reply    {id, text}    reply on a report's thread
//
// Text only. Every message is cut to TEXT_MAX characters here, and every
// place that shows one -- the desk, Studio -- sets it as text, never HTML.

export const TEXT_MAX = 2000;
const ITEMS_KEPT = 50;
const EVERYONE_KEPT = 20;
const EVERYONE_DAYS = 30;
const SENT_KEPT = 200;
const THREAD_KEPT = 60;
const REPORTS_READ = 10;
const EVERYONE_KEY = "inbox-everyone";
const SENT_KEY = "inbox-sent";

const json = (value, status = 200) =>
  new Response(JSON.stringify(value), { status, headers: { "content-type": "application/json" } });
const words = (message, status) =>
  new Response(message, { status, headers: { "content-type": "text/plain; charset=utf-8" } });

async function getJSON(env, key, fallback = null) {
  try {
    return JSON.parse((await env.APPS.get(key)) || "null") ?? fallback;
  } catch (_) {
    return fallback;
  }
}

function newId() {
  const b = crypto.getRandomValues(new Uint8Array(6));
  return [...b].map((x) => x.toString(16).padStart(2, "0")).join("");
}

/// A report's reply secret: 32 hex characters, shown once to the sender.
export function newReportKey() {
  const b = crypto.getRandomValues(new Uint8Array(16));
  return [...b].map((x) => x.toString(16).padStart(2, "0")).join("");
}

export async function keyHash(key) {
  const d = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(String(key)));
  return [...new Uint8Array(d)].map((x) => x.toString(16).padStart(2, "0")).join("");
}

/// Message text as stored: trimmed, line endings made plain, cut to size.
/// Control characters other than newline and tab are dropped.
export function cleanText(value) {
  const s = String(value ?? "")
    .replace(/\r\n?/g, "\n")
    // eslint-disable-next-line no-control-regex
    .replace(/[\u0000-\u0008\u000b-\u001f\u007f]/g, "")
    .trim();
  return s.length > TEXT_MAX ? s.slice(0, TEXT_MAX) : s;
}

/// A report record as the desk lists it: never its reply secrets.
export function deskReport(r) {
  if (!r || typeof r !== "object") return r;
  const { keys: secrets, ...rest } = r;
  return { ...rest, has_key: Array.isArray(secrets) && secrets.length > 0 };
}

/// A report as its sender sees it: what they sent and the conversation.
function personReport(r) {
  return {
    id: r.id,
    received: r.received || 0,
    note: r.note || "",
    session: r.session || "",
    thread: (Array.isArray(r.thread) ? r.thread : []).map((m) => ({ id: m.id, who: m.who === "krate" ? "krate" : "you", text: m.text, at: m.at })),
  };
}

async function ownsReport(r, identity, ref, ownersOf) {
  if (!r) return false;
  if (r.owner && identity && ownersOf(identity).includes(r.owner)) return true;
  const key = ref && typeof ref.key === "string" ? ref.key : "";
  if (!/^[0-9a-f]{32}$/.test(key) || !Array.isArray(r.keys)) return false;
  return r.keys.includes(await keyHash(key));
}

function recentEveryone(list, now) {
  const since = now - EVERYONE_DAYS * 86400000;
  return (Array.isArray(list) ? list : []).filter((m) => m && m.at >= since).slice(0, EVERYONE_KEPT);
}

/// The person's side: /inbox, /inbox/read, /inbox/answer. Null for any
/// other path.
export async function inboxRoute(request, url, env, deps) {
  const { pathname } = url;
  if (request.method !== "POST" || !["/inbox", "/inbox/read", "/inbox/answer"].includes(pathname)) return null;
  const body = await request.json().catch(() => ({}));
  const identity = await deps.verifyGitHub(request, env).catch(() => null);
  const principals = identity ? deps.ownersOf(identity) : [];
  const now = Date.now();

  if (pathname === "/inbox") {
    const boxes = [];
    for (const p of principals) boxes.push(await getJSON(env, `inbox:${p}`, null));
    const everyone = recentEveryone(await getJSON(env, EVERYONE_KEY, []), now);
    const created = identity && identity.id ? Number((await getJSON(env, `user:${identity.id}`, {}) || {}).created) || 0 : 0;
    const sent = Number(body.read);
    const read = Math.max(
      0,
      created,
      Number.isFinite(sent) ? Math.min(sent, now) : 0,
      ...boxes.map((b) => (b && Number(b.read)) || 0),
    );

    const seen = new Set();
    const messages = [];
    const add = (m) => {
      if (!m || !m.id || seen.has(m.id)) return;
      seen.add(m.id);
      messages.push(m);
    };
    for (const b of boxes) for (const m of (b && b.items) || []) add({ id: m.id, at: m.at, text: m.text, kind: m.kind === "report" ? "report" : "you", ...(m.report ? { report: m.report } : {}) });
    for (const m of everyone) add({ id: m.id, at: m.at, text: m.text, kind: "everyone" });

    // Report threads: the ones a reply to this person named, and the ones
    // this browser or computer holds a secret for.
    const wanted = [];
    for (const m of messages) if (m.report && !wanted.some((w) => w.id === m.report)) wanted.push({ id: m.report });
    for (const ref of Array.isArray(body.reports) ? body.reports.slice(0, 20) : []) {
      if (!ref || !/^[0-9a-f]{16}$/.test(String(ref.id || ""))) continue;
      const have = wanted.find((w) => w.id === ref.id);
      if (have) have.key = ref.key;
      else wanted.push({ id: String(ref.id), key: ref.key });
    }
    const reports = [];
    for (const ref of wanted.slice(0, REPORTS_READ)) {
      const r = await getJSON(env, `report:${ref.id}`, null);
      if (!(await ownsReport(r, identity, ref, deps.ownersOf))) continue;
      const shown = personReport(r);
      reports.push(shown);
      // A reply on a report sent signed out reaches nobody's inbox; it is
      // read from the thread, so it is listed from there.
      for (const t of shown.thread) if (t.who === "krate") add({ id: t.id, at: t.at, text: t.text, kind: "report", report: r.id });
    }
    messages.sort((a, b) => (b.at || 0) - (a.at || 0));
    return json({
      signed_in: Boolean(identity),
      read,
      unread: messages.filter((m) => m.at > read).length,
      messages,
      reports,
    });
  }

  if (pathname === "/inbox/read") {
    const at = Math.min(now, Number(body.at) || now);
    // Signed out there is nothing to keep here: the browser keeps its own.
    if (!identity) return json({ ok: true, read: at });
    const primary = deps.ownerOf(identity);
    if (!primary) return json({ ok: true, read: at });
    const box = (await getJSON(env, `inbox:${primary}`, null)) || { items: [] };
    // One write, and only when it moves: opening the inbox twice costs nothing.
    if (!(Number(box.read) >= at)) {
      box.read = at;
      await env.APPS.put(`inbox:${primary}`, JSON.stringify(box));
    }
    return json({ ok: true, read: box.read });
  }

  // /inbox/answer: the person answers on their report's thread.
  const over = deps.rateLimited ? await deps.rateLimited(request, env, "inbox/answer") : null;
  if (over) return over;
  const id = String(body.report || "");
  if (!/^[0-9a-f]{16}$/.test(id)) return words("No such report.", 404);
  const text = cleanText(body.text);
  if (!text) return words("Write something first.", 400);
  const r = await getJSON(env, `report:${id}`, null);
  if (!(await ownsReport(r, identity, { key: body.key }, deps.ownersOf))) return words("No such report.", 404);
  const entry = { id: newId(), who: "them", text, at: now };
  r.thread = [...(Array.isArray(r.thread) ? r.thread : []), entry].slice(-THREAD_KEPT);
  // Back on the desk's pile: it needs the founder again.
  r.state = "new";
  r.answered = now;
  await env.APPS.put(`report:${id}`, JSON.stringify(r));
  if (deps.tellFounder) {
    const telling = deps.tellFounder(env, r, text, identity).catch((error) =>
      console.error(`could not tell the founder about the answer on report ${id}: ${error}`),
    );
    if (deps.ctx && typeof deps.ctx.waitUntil === "function") deps.ctx.waitUntil(telling);
    else await telling;
  }
  return json({ ok: true, report: personReport(r) });
}

/// Who a person is, in the desk's words: their name, login or address.
function labelOf(u) {
  return String(u.name || u.login || u.email || u.id || "");
}

async function eachUser(env, visit) {
  let cursor;
  for (;;) {
    const page = await env.APPS.list({ prefix: "user:", cursor, limit: 1000 });
    for (const { name } of page.keys || []) {
      const u = await getJSON(env, name, null);
      if (u) visit({ ...u, id: u.id || name.slice(5) });
    }
    if (page.list_complete !== false || !page.cursor) break;
    cursor = page.cursor;
  }
}

async function addToInbox(env, principal, item) {
  const box = (await getJSON(env, `inbox:${principal}`, null)) || { items: [] };
  box.items = [item, ...(Array.isArray(box.items) ? box.items : [])].slice(0, ITEMS_KEPT);
  await env.APPS.put(`inbox:${principal}`, JSON.stringify(box));
}

async function noteSent(env, entry) {
  const sent = await getJSON(env, SENT_KEY, []);
  await env.APPS.put(SENT_KEY, JSON.stringify([entry, ...(Array.isArray(sent) ? sent : [])].slice(0, SENT_KEPT)));
}

/// The desk's side, under /admin/api/. `admin` is the verified admin.
/// Null for a route that is not this module's.
export async function deskInboxApi(request, route, env, body, admin) {
  const method = request.method;
  const now = Date.now();

  if (route === "messages" && method === "GET") {
    return json({ sent: await getJSON(env, SENT_KEY, []), everyone_days: EVERYONE_DAYS });
  }

  if (route.startsWith("messages/people") && method === "GET") {
    const q = String(new URL(request.url).searchParams.get("q") || "").trim().toLowerCase();
    if (!q) return json({ people: [] });
    const people = [];
    await eachUser(env, (u) => {
      const hay = [u.id, u.login, u.github_login, u.email, u.name].map((x) => String(x || "").toLowerCase());
      const exact = hay.includes(q);
      if (exact || hay.some((h) => h.includes(q))) {
        people.push({ id: u.id, name: u.name || "", login: u.github_login || u.login || "", email: u.email || "", providers: u.providers || [], exact });
      }
    });
    people.sort((a, b) => Number(b.exact) - Number(a.exact));
    return json({ people: people.slice(0, 12) });
  }

  if (route === "messages/send" && method === "POST") {
    const text = cleanText(body.text);
    if (!text) return words("Write the message first.", 400);
    const id = newId();
    if (body.to === "everyone") {
      const list = await getJSON(env, EVERYONE_KEY, []);
      await env.APPS.put(EVERYONE_KEY, JSON.stringify([{ id, at: now, text }, ...recentEveryone(list, now)].slice(0, EVERYONE_KEPT)));
      await noteSent(env, { id, at: now, to: "everyone", label: "Everyone", text, by: admin.login });
      return json({ ok: true, id, to: "everyone" });
    }
    const userId = String(body.to || "");
    if (!/^[A-Za-z0-9_-]{1,80}$/.test(userId)) return words("Pick someone first, or send it to everyone.", 400);
    const u = await getJSON(env, `user:${userId}`, null);
    if (!u) return words("No account with that id.", 404);
    const principal = `account:${userId}`;
    await addToInbox(env, principal, { id, at: now, text, kind: "you" });
    await noteSent(env, { id, at: now, to: principal, label: labelOf({ ...u, id: userId }), text, by: admin.login });
    return json({ ok: true, id, to: principal });
  }

  if (route === "messages/remove" && method === "POST") {
    const id = String(body.id || "");
    const sent = await getJSON(env, SENT_KEY, []);
    const entry = sent.find((s) => s && s.id === id);
    if (!entry) return words("Nothing sent with that id.", 404);
    if (entry.to === "everyone") {
      const list = await getJSON(env, EVERYONE_KEY, []);
      await env.APPS.put(EVERYONE_KEY, JSON.stringify(list.filter((m) => m.id !== id)));
    } else {
      const box = await getJSON(env, `inbox:${entry.to}`, null);
      if (box && Array.isArray(box.items)) {
        box.items = box.items.filter((m) => m.id !== id);
        await env.APPS.put(`inbox:${entry.to}`, JSON.stringify(box));
      }
      if (entry.report) {
        const r = await getJSON(env, `report:${entry.report}`, null);
        if (r && Array.isArray(r.thread)) {
          r.thread = r.thread.filter((m) => m.id !== id);
          await env.APPS.put(`report:${entry.report}`, JSON.stringify(r));
        }
      }
    }
    await env.APPS.put(SENT_KEY, JSON.stringify(sent.filter((s) => s.id !== id)));
    return json({ ok: true });
  }

  if (route === "report/reply" && method === "POST") {
    const reportId = String(body.id || "");
    if (!/^[0-9a-f]{16}$/.test(reportId)) return words("No such report.", 404);
    const text = cleanText(body.text);
    if (!text) return words("Write the reply first.", 400);
    const r = await getJSON(env, `report:${reportId}`, null);
    if (!r) return words("No such report.", 404);
    const id = newId();
    r.thread = [...(Array.isArray(r.thread) ? r.thread : []), { id, who: "krate", text, at: now, by: admin.login }].slice(-THREAD_KEPT);
    if (!r.state || r.state === "new") r.state = "seen";
    await env.APPS.put(`report:${reportId}`, JSON.stringify(r));
    // Who reads it: the account that sent the report, in its inbox; a
    // report sent signed out is read from its thread by the one browser or
    // computer that holds its secret.
    const to = r.owner || "";
    if (to) await addToInbox(env, to, { id, at: now, text, kind: "report", report: reportId });
    await noteSent(env, { id, at: now, to: to || `report:${reportId}`, label: r.name && r.name !== "anonymous" ? r.name : "Someone without an account", text, by: admin.login, report: reportId });
    return json({
      ok: true,
      id,
      reaches: to ? "account" : (Array.isArray(r.keys) && r.keys.length ? "device" : "nobody"),
      thread: r.thread,
    });
  }

  return null;
}

/// The desk's look for these parts, in the desk's own tokens.
export const DESK_INBOX_STYLE = `
.thread{margin-top:8px;border-top:1px solid var(--line);padding-top:8px}
.thread .msg.them{background:#1a2233}
.thread .msg.krate{background:#15251c;margin-left:32px}
.reach{font-size:12px;color:var(--mut);margin-top:4px}
.choice{display:flex;gap:6px;flex-wrap:wrap;margin:6px 0}
.choice button{background:none;border:1px solid var(--line);color:var(--ink);padding:5px 11px;border-radius:99px;cursor:pointer;font:inherit;font-size:12.5px}
.choice button.on{border-color:var(--acc);color:var(--acc)}
.count{font-size:12px;color:var(--mut);text-align:right}
`;

/// The desk script for messages: the Messages tab, and the thread and reply
/// box on each report card. Uses the desk's api/esc/when/ago/el/show.
export function deskInboxScript() {
  return String.raw`
// ---- in-app messages (inbox.js) ----------------------------------------
const MSG_MAX=${TEXT_MAX};
function counted(ta,out){const f=()=>{out.textContent=ta.value.length+" / "+MSG_MAX;};ta.maxLength=MSG_MAX;ta.addEventListener("input",f);f();}
// The thread on a report card, and the box to reply. Who will read it is
// said beside the button, because a report sent signed out has no account.
function reportThreadInto(card,r){
  const box=document.createElement("div");box.className="thread";
  const draw=thread=>{
    const T=thread||[];
    const reach=r.owner?"They read it in Studio, signed in as @"+esc(r.from)+".":(r.has_key?"Sent without an account: they read it in the Studio they sent it from.":"Sent without an account by an older Studio: nobody can read a reply.");
    box.innerHTML=(T.length?T.map(m=>'<div class="msg '+(m.who==="krate"?"krate":"them")+'">'+esc(m.text)+'<div class="mut">'+(m.who==="krate"?"you":"them")+" · "+when(m.at)+'</div></div>').join(""):'<p class="mut">No replies yet.</p>')
      +'<div class="row" style="margin-top:8px"><textarea class="rtext" placeholder="Reply in Studio…"></textarea></div>'
      +'<div class="row" style="margin-top:6px"><button class="act rsend">Send reply</button><span class="reach grow">'+reach+'</span><span class="count rcount"></span></div>';
    const ta=box.querySelector(".rtext");counted(ta,box.querySelector(".rcount"));
    const send=box.querySelector(".rsend");
    send.onclick=async()=>{const x=ta.value.trim();if(!x)return;send.disabled=true;
      try{const out=await api("report/reply",{method:"POST",body:JSON.stringify({id:r.id,text:x})});r.thread=out.thread;draw(out.thread);}
      catch(e){send.disabled=false;alert("The reply did not go: "+e.message);}};
  };
  draw(r.thread);
  card.appendChild(box);
}
async function deskMessages(v){
  const {sent,everyone_days}=await api("messages");
  let to="everyone",person=null;
  v.innerHTML='<div class="card"><b>Send a message in Studio</b><p class="mut" style="margin:4px 0 8px">It shows in Krate Studio, on the web and on computers, however the person signed in. To everyone: anyone who opens Studio in the next '+everyone_days+' days sees it. To one person: only they do.</p>'
    +'<div class="choice"><button data-to="everyone" class="on">Everyone</button><button data-to="one">One person</button></div>'
    +'<div class="pick" style="display:none"><input class="q" placeholder="Their login, email, name or account id"><div class="found"></div></div>'
    +'<textarea class="mtext" placeholder="What they will read…" style="margin-top:6px"></textarea>'
    +'<div class="row" style="margin-top:6px"><button class="act msend">Send</button><span class="mut grow mnote"></span><span class="count mcount"></span></div></div>'
    +'<h3 style="margin:14px 0 8px">Sent ('+sent.length+')</h3><div class="sent"></div>';
  const pick=v.querySelector(".pick"),found=v.querySelector(".found"),note=v.querySelector(".mnote"),ta=v.querySelector(".mtext");
  counted(ta,v.querySelector(".mcount"));
  const label=()=>{note.textContent=to==="everyone"?"To everyone.":(person?"To "+(person.name||person.login||person.email)+(person.login?" (@"+person.login+")":"")+".":"Pick the person first.");};
  v.querySelectorAll(".choice button").forEach(b=>b.onclick=()=>{to=b.dataset.to;v.querySelectorAll(".choice button").forEach(x=>x.classList.toggle("on",x===b));pick.style.display=to==="one"?"":"none";label();});
  let timer=0;
  v.querySelector(".q").oninput=e=>{clearTimeout(timer);const q=e.target.value.trim();timer=setTimeout(async()=>{
    if(!q){found.innerHTML="";return;}
    const {people}=await api("messages/people?q="+encodeURIComponent(q));
    found.innerHTML=people.length?"":'<p class="mut">Nobody matches.</p>';
    for(const p of people){const b=document.createElement("button");b.className="ghost";b.style.margin="6px 6px 0 0";
      b.textContent=(p.name||p.login||p.email||p.id)+(p.login?" @"+p.login:"")+(p.email?" · "+p.email:"")+" · "+(p.providers||[]).join(", ");
      b.onclick=()=>{person=p;found.querySelectorAll("button").forEach(x=>{x.className=x===b?"act":"ghost";});label();};
      found.appendChild(b);}
  },250);};
  label();
  const btn=v.querySelector(".msend");
  btn.onclick=async()=>{const x=ta.value.trim();if(!x)return;
    if(to==="one"&&!person){label();return;}
    const who=to==="everyone"?"everyone who opens Studio":(person.name||person.login||person.email);
    if(!confirm("Send this to "+who+"?\n\n"+x))return;
    btn.disabled=true;
    try{await api("messages/send",{method:"POST",body:JSON.stringify({to:to==="everyone"?"everyone":person.id,text:x})});show("messages");}
    catch(e){btn.disabled=false;alert("It did not go: "+e.message);}};
  const list=v.querySelector(".sent");
  list.innerHTML=sent.length?"":'<p class="mut">Nothing sent yet.</p>';
  for(const s of sent){const d=document.createElement("div");d.className="card";
    d.innerHTML='<div class="row"><b>'+esc(s.label||s.to)+'</b>'+(s.to==="everyone"?'<span class="pill">everyone</span>':s.report?'<span class="pill">report '+esc(s.report)+'</span>':'')+'<span class="grow"></span><span class="mut">'+esc(s.by||"")+" · "+ago(s.at)+'</span><button class="ghost rm">Remove</button></div><div class="msg krate" style="margin-left:0">'+esc(s.text)+'</div>';
    d.querySelector(".rm").onclick=async()=>{if(!confirm("Take this message back? It leaves Studio for everyone it was sent to."))return;await api("messages/remove",{method:"POST",body:JSON.stringify({id:s.id})});show("messages");};
    list.appendChild(d);}
}
`;
}
