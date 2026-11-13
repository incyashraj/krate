// Announcements: the founder's own mail to the people who use Krate,
// written and sent from the desk's Mail section (or with curl).
//
// NON-ESSENTIAL (mail-prefs.js): every one skips an account that turned
// Krate's emails off, ends with that person's own unsubscribe link and
// carries the RFC 8058 one-click headers. The founder writes only the body:
// plain paragraphs, a blank line between them, links as they are, and
// {first_name} where the first name goes ("there" when the account has
// none). The hub makes the HTML and the text, and adds the signature and
// the unsubscribe line itself.
//
// From "Yashraj Pardeshi <yashraj@krate.tech>" through Resend, replies to
// the same address: every mail to a person goes from the hub, never from a
// script on someone's computer.
//
// Each announcement has an id. Its record (KV `announce:<id>`) freezes who
// it is for when the first send is pressed and keeps one result per person:
// sent, skipped (and why) or failed (and why). Each inbox is marked
// (`announced:<id>:<inbox>`) before its mail goes, so a second click, a
// retry, a reload or the cron never mails anyone twice. The same id with a
// different mail is refused.
//
// Pacing. Resend's free plan sends 100 a day and 2 a second, and sign-in
// links share that account. Announcements take their slots from the same
// hub-wide daily ceiling the app mails use (MOMENTS_PER_DAY, 50), so
// sign-in always keeps at least half the day; at most PER_RUN go in one
// run (a Worker request may make 50 outside calls), GAP_MS apart. What is
// left waits in `announce-queue`, and the ten-minute cron sends it as the
// ceiling allows.
//
//   desk  /admin/api/mail/...          (adminApi, KRATE_ADMINS)
//     GET  mail/people                 accounts, who gets mail, counts
//     GET  mail/draft  POST mail/draft the one draft, kept in KV
//     POST mail/test                   to the founder only, "[test] " subject
//     POST mail/send                   {id, subject, body, audience, selected}
//     GET  mail/history                past announcements with counts
//     GET  mail/announcement?id=       one, with what was sent and to whom
//   curl POST /admin/mail/announce     {id, subject, body, audience?, dry_run?}
//                                      dry run by default: counts, never addresses

import { sendMail, accountAddress, daySlot } from "./mail.js";
import { wantsNews, unsubscribeLink, unsubscribeHeaders } from "./mail-prefs.js";

export const FROM = "Yashraj Pardeshi <yashraj@krate.tech>";
export const REPLY_TO = "yashraj@krate.tech";
export const TEST_TO = "yashraj@krate.tech";
/// Where a test email's unsubscribe line goes: the setting, never a real
/// one-click link.
const TEST_UNSUBSCRIBE_LINK = "https://krate.tech/account/#mail";
export const PER_RUN = 40;
const GAP_MS = 600;
const DRAFT_KEY = "announce-draft";
const QUEUE_KEY = "announce-queue";
const INDEX_KEY = "announce-index";

/// The mail one person gets: subject, text and HTML. Self-contained on
/// purpose -- the desk's live preview runs this very function (its source
/// is put into the page), so the preview is exactly what is sent.
export function renderAnnouncement({ subject, body }, { firstName, link }) {
  const LOGO = "https://krate.tech/krate-logo.png";
  const name = String(firstName || "").trim() || "there";
  const esc = (s) => String(s).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
  const fill = (s) => String(s || "").replace(/\{first_name\}/g, name);
  const words = fill(body).replace(/\r\n?/g, "\n").replace(/^\s+|\s+$/g, "");
  const paragraphs = words ? words.split(/\n[ \t]*\n+/) : [];
  const linked = (s) =>
    esc(s).replace(/https?:\/\/[^\s<]*[^\s<.,;:!?)\]'"]/g, (u) => `<a href="${u}" style="color:#3d6df0;">${u}</a>`);
  const unsub = `Don't want these emails? Unsubscribe: ${link}`;
  const text = `${paragraphs.join("\n\n")}

--
best,
Yashraj
krate.tech
We value your contribution to Krate.

${unsub}
`;
  const P = 'style="margin:0 0 16px 0;"';
  const html = `<!doctype html>
<html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><meta name="color-scheme" content="light dark"><meta name="supported-color-schemes" content="light dark"></head>
<body style="margin:0;padding:0;">
<div style="max-width:600px;margin:0 auto;padding:24px 20px;font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif;font-size:15px;line-height:1.6;">
${paragraphs.map((p) => `<p ${P}>${linked(p).replace(/\n/g, "<br>")}</p>`).join("\n")}
<p style="margin:24px 0 0 0;">--<br>best,<br>Yashraj<br><a href="https://krate.tech" style="color:#3d6df0;">krate.tech</a></p>
<p style="margin:10px 0 0 0;"><img src="${LOGO}" width="40" height="40" alt="Krate" style="display:block;width:40px;height:40px;border:0;outline:none;text-decoration:none;"></p>
<p style="margin:10px 0 0 0;"><em>We value your contribution to Krate.</em></p>
<p style="margin:28px 0 0 0;font-size:12px;line-height:1.5;color:#8a8a92;">Don't want these emails? <a href="${esc(link)}" style="color:#8a8a92;text-decoration:underline;">Unsubscribe</a></p>
</div>
</body></html>`;
  return { subject: fill(subject).trim(), text, html };
}

/// The first word of the name an account gave, or "" (rendered "there").
export function firstNameOf(user) {
  return String((user && user.name) || "").trim().split(/\s+/)[0] || "";
}

async function digest(text, n = 16) {
  const d = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(String(text)));
  return [...new Uint8Array(d)].map((b) => b.toString(16).padStart(2, "0")).join("").slice(0, n);
}

const json = (value, status = 200) =>
  new Response(JSON.stringify(value), { status, headers: { "content-type": "application/json" } });
const getJSON = async (env, key, fallback = null) => {
  try {
    return JSON.parse((await env.APPS.get(key)) || "null") ?? fallback;
  } catch (_) {
    return fallback;
  }
};

/// Every account, through KV's pages: `{id, user}`.
async function accounts(env) {
  const out = [];
  let cursor;
  for (;;) {
    const page = await env.APPS.list({ prefix: "user:", cursor });
    for (const { name } of page.keys || []) {
      const user = await getJSON(env, name);
      if (user) out.push({ id: name.slice("user:".length), user });
    }
    if (page.list_complete !== false || !page.cursor) break;
    cursor = page.cursor;
  }
  return out;
}

/// Everyone, for the desk's picker: who gets mail and who does not.
export async function peopleList(env) {
  const people = (await accounts(env)).map(({ id, user }) => ({
    id,
    name: user.name || "",
    login: user.login || "",
    email: accountAddress(user) || "",
    news: wantsNews(user),
    created: user.created || 0,
  }));
  people.sort((a, b) => b.created - a.created);
  const counts = {
    accounts: people.length,
    gets_mail: people.filter((p) => p.email && p.news).length,
    turned_off: people.filter((p) => p.email && !p.news).length,
    no_address: people.filter((p) => !p.email).length,
  };
  return { people, counts };
}

/// Check what an announcement says; the cleaned fields or an error.
function checked(input) {
  const id = String(input.id || "");
  const subject = String(input.subject || "").trim();
  const body = String(input.body ?? input.text ?? "").trim();
  const audience = input.audience === "selected" ? "selected" : "all";
  const selected = Array.isArray(input.selected) ? [...new Set(input.selected.map(String))].slice(0, 5000) : [];
  if (!/^[a-z0-9][a-z0-9-]{0,47}$/.test(id)) return { error: "bad-id", message: "id: lower-case letters, digits and dashes, like thank-you-2026-11." };
  if (!subject || subject.length > 200) return { error: "bad-subject", message: "Write a subject (up to 200 characters)." };
  if (!body || body.length > 20000) return { error: "bad-body", message: "Write the email (up to 20,000 characters)." };
  if (audience === "selected" && !selected.length) return { error: "nobody", message: "Pick at least one person, or send to everyone." };
  return { id, subject, body, audience, selected };
}

/// Who an announcement goes to, frozen when it starts: every account with
/// an address (the turned-off ones are recorded as skipped when the run
/// reaches them), or the people picked.
async function targetsFor(env, a) {
  const { people } = await peopleList(env);
  const byId = new Map(people.map((p) => [p.id, p]));
  const ids = a.audience === "selected" ? a.selected.filter((id) => byId.has(id)) : people.map((p) => p.id);
  const targets = ids.filter((id) => byId.get(id).email);
  // Who already turned it off is skipped from the start; anyone who turns
  // it off later is skipped when the run reaches them.
  const results = {};
  for (const id of targets) if (!byId.get(id).news) results[id] = { s: "skipped", why: "unsubscribed", at: Date.now() };
  return { targets, results, no_address: ids.length - targets.length };
}

/// The record for `a`, made when it is first sent. Refuses the same id
/// with a different mail.
async function begin(env, a, by) {
  const fingerprint = await digest(`${a.subject}\n${a.body}`);
  const held = await getJSON(env, `announce:${a.id}`);
  if (held) {
    if (held.fingerprint !== fingerprint) return { error: "id-used", message: "This email was already sent with different words. Start a new one." };
    return { record: held };
  }
  const { targets, results, no_address } = await targetsFor(env, a);
  const record = {
    id: a.id,
    subject: a.subject,
    body: a.body,
    fingerprint,
    audience: a.audience,
    created: Date.now(),
    by: by || "",
    targets,
    no_address,
    results,
    status: "sending",
  };
  await env.APPS.put(`announce:${a.id}`, JSON.stringify(record));
  const index = await getJSON(env, INDEX_KEY, []);
  if (!index.includes(a.id)) await env.APPS.put(INDEX_KEY, JSON.stringify([a.id, ...index].slice(0, 500)));
  return { record };
}

/// Counts for a record: sent, skipped, failed, and still waiting.
export function countsOf(record) {
  const c = { people: record.targets.length, sent: 0, skipped: 0, failed: 0, waiting: 0 };
  for (const id of record.targets) {
    const r = record.results[id];
    if (!r) c.waiting += 1;
    else if (r.s === "sent") c.sent += 1;
    else if (r.s === "skipped") c.skipped += 1;
    else c.failed += 1;
  }
  return c;
}

async function setQueued(env, id, queued) {
  const queue = await getJSON(env, QUEUE_KEY, []);
  const has = queue.includes(id);
  if (queued && !has) await env.APPS.put(QUEUE_KEY, JSON.stringify([...queue, id]));
  if (!queued && has) await env.APPS.put(QUEUE_KEY, JSON.stringify(queue.filter((q) => q !== id)));
}

/// Send what is left of one announcement, as far as this run and today's
/// ceiling allow. Answers the record, with `stopped` saying why it paused.
export async function runAnnouncement(env, id, { sleep = (ms) => new Promise((r) => setTimeout(r, ms)) } = {}) {
  const record = await getJSON(env, `announce:${id}`);
  if (!record) return null;
  const perRun = Math.min(PER_RUN, Number(env.ANNOUNCE_PER_RUN) || PER_RUN);
  const gap = env.ANNOUNCE_GAP_MS !== undefined ? Number(env.ANNOUNCE_GAP_MS) : GAP_MS;
  let stopped = null;
  let tried = 0;
  const at = () => Date.now();
  for (const accountId of record.targets) {
    if (record.results[accountId]) continue;
    const user = await getJSON(env, `user:${accountId}`);
    const to = accountAddress(user);
    if (!user || !to) {
      record.results[accountId] = { s: "skipped", why: "no address", at: at() };
      continue;
    }
    // Asked at the moment of sending: an unsubscribe a minute ago holds.
    if (!wantsNews(user)) {
      record.results[accountId] = { s: "skipped", why: "unsubscribed", at: at() };
      continue;
    }
    const mark = `announced:${id}:${await digest(String(to).toLowerCase())}`;
    const marked = await env.APPS.get(mark);
    if (marked) {
      record.results[accountId] =
        marked === accountId ? { s: "sent", at: at() } : { s: "skipped", why: "same address as another account", at: at() };
      continue;
    }
    if (tried >= perRun) {
      stopped = `one run sends at most ${perRun}`;
      break;
    }
    if (!env.RESEND_API_KEY) {
      stopped = "this hub has no RESEND_API_KEY";
      break;
    }
    const link = await unsubscribeLink(env, accountId);
    if (!link) {
      stopped = "this hub has no KEY_WRAP_SECRET, so no unsubscribe link can be made";
      break;
    }
    if (!(await daySlot(env))) {
      stopped = "today's email limit is reached";
      break;
    }
    // Marked first: whatever happens next, this inbox is not mailed twice.
    await env.APPS.put(mark, accountId);
    if (tried > 0 && gap > 0) await sleep(gap);
    tried += 1;
    const mail = renderAnnouncement(record, { firstName: firstNameOf(user), link });
    const why = await sendMail(env, { to, ...mail, headers: unsubscribeHeaders(link), from: FROM, replyTo: REPLY_TO });
    if (!why) {
      record.results[accountId] = { s: "sent", at: at() };
      continue;
    }
    console.error(`an announcement did not go: ${why}`);
    // Resend said no, so it did not go: the mark comes off.
    await env.APPS.delete(mark).catch(() => {});
    if (/^resend (429|5\d\d)|could not reach/.test(why)) {
      // Busy or down: this person waits for the next run.
      stopped = "Resend is busy; the rest go on the next run";
      break;
    }
    record.results[accountId] = { s: "failed", why: why.slice(0, 160), at: at() };
  }
  const counts = countsOf(record);
  record.status = counts.waiting ? "queued" : "done";
  record.updated = Date.now();
  if (stopped) record.stopped = stopped;
  else delete record.stopped;
  await env.APPS.put(`announce:${id}`, JSON.stringify(record));
  await setQueued(env, id, Boolean(counts.waiting));
  return record;
}

/// The ten-minute cron: carry on with anything waiting. One KV read when
/// nothing is.
export async function runQueuedAnnouncements(env) {
  // One announcement a run: a run may make only so many outside calls.
  const [id] = await getJSON(env, QUEUE_KEY, []);
  if (!id) return null;
  const record = await runAnnouncement(env, id);
  if (!record) await setQueued(env, id, false);
  return record;
}

/// A record as the desk shows it.
function summary(record) {
  return {
    id: record.id,
    subject: record.subject,
    created: record.created,
    updated: record.updated || record.created,
    audience: record.audience,
    status: record.status,
    stopped: record.stopped || null,
    no_address: record.no_address || 0,
    counts: countsOf(record),
  };
}

/// The account the admin signs in with, if their GitHub login has one.
async function adminAccount(env, admin) {
  const id = admin && admin.login ? await env.APPS.get(`ghlogin:${String(admin.login).toLowerCase()}`) : null;
  const user = id ? await getJSON(env, `user:${id}`) : null;
  return user ? { id, user } : null;
}

/// The desk's /admin/api/mail/* routes. `admin` is the verified admin.
export async function deskMailApi(request, route, env, body, admin, opts = {}) {
  const method = request.method;
  if (route === "mail/people" && method === "GET") return json(await peopleList(env));
  if (route === "mail/draft" && method === "GET") return json({ draft: await getJSON(env, DRAFT_KEY) });
  if (route === "mail/draft" && method === "POST") {
    const held = await getJSON(env, DRAFT_KEY);
    const draft = {
      id: (held && held.id) || `mail-${new Date().toISOString().slice(0, 10)}-${(await digest(crypto.randomUUID())).slice(0, 6)}`,
      subject: String(body.subject || "").slice(0, 200),
      body: String(body.body || "").slice(0, 20000),
      audience: body.audience === "selected" ? "selected" : "all",
      selected: Array.isArray(body.selected) ? body.selected.map(String).slice(0, 5000) : [],
      saved: Date.now(),
    };
    await env.APPS.put(DRAFT_KEY, JSON.stringify(draft));
    return json({ draft });
  }
  if (route === "mail/draft" && method === "DELETE") {
    await env.APPS.delete(DRAFT_KEY);
    return json({ ok: true });
  }
  if (route === "mail/test" && method === "POST") {
    const subject = String(body.subject || "").trim();
    const words = String(body.body || "").trim();
    if (!subject || !words) return json({ error: "empty", message: "Write a subject and the email first." }, 400);
    const mine = await adminAccount(env, admin);
    // A test's unsubscribe line points at the email setting, not at the
    // founder's own one-click link, and carries no one-click headers: a click
    // while checking how the email looks must not stop his own email.
    const link = TEST_UNSUBSCRIBE_LINK;
    const mail = renderAnnouncement({ subject, body: words }, { firstName: mine ? firstNameOf(mine.user) : "Yashraj", link });
    const why = await sendMail(env, {
      to: TEST_TO,
      subject: `[test] ${mail.subject}`,
      text: mail.text,
      html: mail.html,
      from: FROM,
      replyTo: REPLY_TO,
    });
    return why ? json({ error: "not-sent", message: `The test did not go: ${why}` }, 502) : json({ sent: true, to: TEST_TO });
  }
  if (route === "mail/send" && method === "POST") {
    const a = checked(body);
    if (a.error) return json(a, 400);
    const started = await begin(env, a, admin && admin.login);
    if (started.error) return json(started, 409);
    const draft = await getJSON(env, DRAFT_KEY);
    if (draft && draft.id === a.id) await env.APPS.delete(DRAFT_KEY);
    const record = await runAnnouncement(env, a.id, opts);
    return json({ ...summary(record), results: record.results });
  }
  if (route === "mail/history" && method === "GET") {
    const index = await getJSON(env, INDEX_KEY, []);
    const out = [];
    for (const id of index.slice(0, 100)) {
      const record = await getJSON(env, `announce:${id}`);
      if (record) out.push(summary(record));
    }
    return json({ announcements: out });
  }
  if (route.startsWith("mail/announcement") && method === "GET") {
    const id = new URL(request.url).searchParams.get("id") || "";
    const record = await getJSON(env, `announce:${id}`);
    if (!record) return json({ error: "not-found", message: "No such email." }, 404);
    return json({ ...summary(record), body: record.body, targets: record.targets, results: record.results });
  }
  return null;
}

/// POST /admin/mail/announce, for curl. Dry run unless `"dry_run": false`;
/// a dry run changes nothing and names nobody.
export async function announceRoute(request, env, { isAdmin, sleep } = {}) {
  const admin = await isAdmin(request, env);
  // Exactly what every other admin route answers a stranger.
  if (!admin) return new Response("not found", { status: 404 });
  const input = await request.json().catch(() => null);
  if (!input) return json({ error: "bad-request", message: "Send JSON: {id, subject, body, dry_run}." }, 400);
  const a = checked(input);
  if (a.error) return json(a, 400);
  if (input.dry_run !== false) {
    const held = await getJSON(env, `announce:${a.id}`);
    const { people } = await peopleList(env);
    const chosen = a.audience === "selected" ? people.filter((p) => a.selected.includes(p.id)) : people;
    const skipped = { unsubscribed: 0, no_address: 0, already_sent: 0, same_address: 0 };
    const inboxes = new Set();
    let would = 0;
    for (const p of chosen) {
      if (!p.email) {
        skipped.no_address += 1;
        continue;
      }
      if (!p.news) {
        skipped.unsubscribed += 1;
        continue;
      }
      const inbox = await digest(p.email.toLowerCase());
      if (inboxes.has(inbox)) {
        skipped.same_address += 1;
        continue;
      }
      inboxes.add(inbox);
      if (held && (await env.APPS.get(`announced:${a.id}:${inbox}`))) {
        skipped.already_sent += 1;
        continue;
      }
      would += 1;
    }
    return json({ dry_run: true, id: a.id, accounts: chosen.length, would_send: would, skipped, one_run_sends_at_most: PER_RUN });
  }
  const started = await begin(env, a, admin.login);
  if (started.error) return json(started, 409);
  const record = await runAnnouncement(env, a.id, { sleep });
  return json({ dry_run: false, ...summary(record) });
}

// ------------------------------------------------------------- the desk tab
//
// The desk's Mail section, put into adminPage's script. It shares the
// page's `HUB`, `TOKEN`, `el`, `esc` and `when`. Written without backticks
// or dollar-brace so it can sit inside the page's template literal.

export function deskMailScript() {
  return `const renderAnnouncement=${renderAnnouncement.toString()};\n` + DESK_MAIL;
}

const DESK_MAIL = String.raw`
const mapi=(p,opt)=>fetch(HUB+"/admin/api/"+p,{...opt,headers:{authorization:"Bearer "+TOKEN,"content-type":"application/json"}}).then(async r=>{let b=null;try{b=await r.json()}catch(e){}return{ok:r.ok,status:r.status,body:b||{}}});
const MAIL={people:[],counts:{},draft:null,saveT:null,view:"text"};
const firstOf=n=>String(n||"").trim().split(/\s+/)[0]||"";
const SAMPLE_LINK=HUB+"/mail/unsubscribe?u=…&t=…";
// The preview shows the mail as a light mail app does; the mail itself sets
// no colours, so a dark mail app shows it light on dark.
const asLight=h=>h.replace("<head>","<head><style>:root{color-scheme:light}body{background:#fff;color:#1c1c1e}</style>");
async function deskMail(v){
  v.innerHTML='<p class="mut">loading…</p>';
  const [P,D,H]=await Promise.all([mapi("mail/people"),mapi("mail/draft"),mapi("mail/history")]);
  MAIL.people=P.body.people||[];MAIL.counts=P.body.counts||{};
  MAIL.draft=D.body.draft||{subject:"",body:"",audience:"all",selected:[]};
  const c=MAIL.counts;
  v.innerHTML='<div class="mailgrid">'
   +'<div class="card mailc"><div class="row"><b>New email</b><span class="grow"></span><span class="mut" id="mSaved"></span></div>'
   +'<label class="mut lab" for="mSubj">Subject</label><input id="mSubj" placeholder="Thank you for trying Krate">'
   +'<label class="mut lab" for="mBody">Email</label><textarea id="mBody" placeholder="Hi {first_name},\n\nWrite here. A blank line starts a new paragraph."></textarea>'
   +'<p class="mut hint">A blank line starts a new paragraph. Links become links. <code>{first_name}</code> becomes their first name, or “there”. The signature and the unsubscribe line are added for you.</p>'
   +'<div class="lab mut">Send to</div>'
   +'<label class="aud"><input type="radio" name="aud" value="all"> <span><b>Everyone who gets emails</b> · '+c.gets_mail+' people<br><span class="mut">'+c.turned_off+' unsubscribed and '+c.no_address+' with no address are left out</span></span></label>'
   +'<label class="aud"><input type="radio" name="aud" value="selected"> <span><b>Selected people</b> · <span id="mPicked">0</span> picked</span></label>'
   +'<div id="mPick" class="pick" style="display:none"><input id="mQ" placeholder="Search name, login or email…"><div id="mList" class="plist"></div></div>'
   +'<div class="row" style="margin-top:14px"><button class="ghost" id="mTest">Send me a test</button><span class="grow"></span><button class="act" id="mSend">Send…</button></div>'
   +'<p class="mut" id="mNote" style="margin-top:8px"></p><div id="mConfirm"></div><div id="mResult"></div></div>'
   +'<div class="card mailp"><div class="row"><b>What they get</b><span class="grow"></span><button class="ghost sm on" data-pv="html">Email</button><button class="ghost sm" data-pv="text">Plain text</button></div>'
   +'<div class="mut" style="margin:8px 0 2px">From Yashraj Pardeshi &lt;yashraj@krate.tech&gt; · to <span id="pvTo"></span></div>'
   +'<div class="pvsubj" id="pvSubj"></div><iframe id="pvHtml" title="Preview of the email" sandbox="allow-same-origin"></iframe><pre id="pvText" style="display:none"></pre></div>'
   +'</div><h3 style="margin:22px 0 8px">Sent</h3><div id="mHist"></div>';
  const d=MAIL.draft;el("#mSubj").value=d.subject||"";el("#mBody").value=d.body||"";
  document.querySelectorAll('input[name=aud]').forEach(r=>{r.checked=r.value===(d.audience||"all");r.onchange=()=>{audienceChanged();save();preview();};});
  MAIL.sel=new Set(d.selected||[]);
  el("#mSubj").oninput=()=>{preview();save();};el("#mBody").oninput=()=>{preview();save();};
  el("#mQ").oninput=pickList;
  document.querySelectorAll("[data-pv]").forEach(b=>b.onclick=()=>{MAIL.pv=b.dataset.pv;document.querySelectorAll("[data-pv]").forEach(x=>x.classList.toggle("on",x===b));preview();});
  el("#mTest").onclick=sendTest;el("#mSend").onclick=confirmSend;
  if(d.saved)el("#mSaved").textContent="Draft saved "+when(d.saved);
  audienceChanged();preview();mailHistory(H.body.announcements||[]);
}
function aud(){const r=document.querySelector('input[name=aud]:checked');return r?r.value:"all";}
function audienceChanged(){el("#mPick").style.display=aud()==="selected"?"":"none";pickList();}
function pickList(){
  const q=(el("#mQ").value||"").toLowerCase();
  const rows=MAIL.people.filter(p=>!q||(p.name+" "+p.login+" "+p.email).toLowerCase().includes(q));
  el("#mList").innerHTML=rows.map(p=>{const off=!p.news||!p.email;return '<label class="prow'+(off?' off':'')+'"><input type="checkbox" data-id="'+esc(p.id)+'"'+(MAIL.sel.has(p.id)&&!off?' checked':'')+(off?' disabled':'')+'> <span class="grow"><b>'+esc(p.name||p.login||p.email||"(no name)")+'</b> <span class="mut">'+esc(p.login?"@"+p.login:"")+' '+esc(p.email||"no address")+'</span></span>'+(!p.email?'<span class="pill">no address</span>':!p.news?'<span class="pill">unsubscribed</span>':'')+'</label>';}).join("")||'<p class="mut">Nobody matches.</p>';
  el("#mList").querySelectorAll("input[data-id]").forEach(b=>b.onchange=()=>{b.checked?MAIL.sel.add(b.dataset.id):MAIL.sel.delete(b.dataset.id);el("#mPicked").textContent=picked().length;save();preview();});
  el("#mPicked").textContent=picked().length;
}
function picked(){const ok=new Set(MAIL.people.filter(p=>p.news&&p.email).map(p=>p.id));return [...MAIL.sel].filter(id=>ok.has(id));}
function recipients(){return aud()==="selected"?MAIL.people.filter(p=>picked().includes(p.id)):MAIL.people.filter(p=>p.news&&p.email);}
function preview(){
  const r=recipients();const who=r[0];
  el("#pvTo").textContent=who?(who.name||who.login||who.email)+(r.length>1?" and "+(r.length-1)+" more, each with their own name and link":""):"nobody yet";
  const m=renderAnnouncement({subject:el("#mSubj").value,body:el("#mBody").value},{firstName:who?firstOf(who.name):"",link:SAMPLE_LINK});
  el("#pvSubj").textContent=m.subject||"(no subject)";
  const html=MAIL.pv!=="text";el("#pvHtml").style.display=html?"":"none";el("#pvText").style.display=html?"none":"";
  el("#pvHtml").srcdoc=asLight(m.html);el("#pvText").textContent=m.text;
}
function draftNow(){return{subject:el("#mSubj").value,body:el("#mBody").value,audience:aud(),selected:[...MAIL.sel]};}
function save(){
  clearTimeout(MAIL.saveT);el("#mSaved").textContent="Saving…";
  MAIL.saveT=setTimeout(async()=>{const r=await mapi("mail/draft",{method:"POST",body:JSON.stringify(draftNow())});if(r.ok){MAIL.draft=r.body.draft;el("#mSaved").textContent="Draft saved";}else el("#mSaved").textContent="Not saved ("+r.status+")";},1200);
}
async function sendTest(){
  const n=el("#mNote");n.textContent="Sending you a test…";
  const r=await mapi("mail/test",{method:"POST",body:JSON.stringify(draftNow())});
  n.textContent=r.ok?"Test sent to "+r.body.to+". It does not count as sending this email.":(r.body.message||"The test did not go ("+r.status+").");
}
function confirmSend(){
  const n=recipients().length,s=el("#mSubj").value.trim(),b=el("#mBody").value.trim();
  if(!s||!b){el("#mNote").textContent="Write a subject and the email first.";return;}
  if(!n){el("#mNote").textContent="Nobody to send to. Pick at least one person.";return;}
  el("#mConfirm").innerHTML='<div class="confirm"><p>Send <b>“'+esc(s)+'”</b> to <b>'+n+(n===1?' person':' people')+'</b>? Each gets it once, with their own unsubscribe link.</p><div class="row" style="margin-top:10px"><button class="act" id="mGo">Send to '+n+'</button><button class="ghost" id="mNo">Cancel</button></div></div>';
  el("#mNo").onclick=()=>{el("#mConfirm").innerHTML="";};
  el("#mGo").onclick=async()=>{
    el("#mGo").disabled=true;el("#mGo").textContent="Sending…";clearTimeout(MAIL.saveT);
    const id=(MAIL.draft&&MAIL.draft.id)||"";
    if(!id){const r=await mapi("mail/draft",{method:"POST",body:JSON.stringify(draftNow())});MAIL.draft=r.body.draft;}
    const r=await mapi("mail/send",{method:"POST",body:JSON.stringify({id:MAIL.draft.id,...draftNow()})});
    el("#mConfirm").innerHTML="";
    if(!r.ok){el("#mNote").textContent=r.body.message||"It did not send ("+r.status+").";return;}
    el("#mResult").innerHTML=resultHtml(r.body);
    const H=await mapi("mail/history");mailHistory(H.body.announcements||[]);
    el("#mSubj").value="";el("#mBody").value="";MAIL.sel=new Set();MAIL.draft=null;el("#mSaved").textContent="";pickList();preview();
  };
}
function nameOf(id){const p=MAIL.people.find(x=>x.id===id);return p?(p.name||p.login||p.email):id;}
function resultHtml(a){
  const c=a.counts,res=a.results||{};
  const rows=Object.keys(res).map(id=>{const x=res[id];return '<div class="prow"><span class="grow">'+esc(nameOf(id))+'</span><span class="pill '+(x.s==="sent"?"active":x.s==="failed"?"open":"")+'">'+esc(x.s)+(x.why?": "+esc(x.why):"")+'</span></div>';}).join("");
  return '<div class="confirm"><b>'+c.sent+' sent</b> · '+c.skipped+' skipped · '+c.failed+' failed'+(c.waiting?' · <b>'+c.waiting+' waiting</b>':'')
    +(c.waiting?'<p class="mut" style="margin-top:6px">'+esc(a.stopped||"")+'. The rest go out by themselves on the ten-minute run, within the day’s limit.</p>':'')
    +'<div style="margin-top:8px">'+rows+'</div></div>';
}
function mailHistory(list){
  const h=el("#mHist");if(!h)return;
  h.innerHTML=list.length?"":'<p class="mut">Nothing sent yet.</p>';
  for(const a of list){
    const d=document.createElement("div");d.className="card";
    d.innerHTML='<div class="row" style="cursor:pointer"><b>'+esc(a.subject)+'</b><span class="pill '+(a.status==="done"?"active":"open")+'">'+(a.status==="done"?"sent":"sending")+'</span><span class="grow"></span><span class="mut">'+a.counts.sent+' sent · '+a.counts.skipped+' skipped · '+a.counts.failed+' failed'+(a.counts.waiting?' · '+a.counts.waiting+' waiting':'')+' · '+when(a.created)+'</span></div><div class="hbox"></div>';
    d.querySelector(".row").onclick=async()=>{
      const box=d.querySelector(".hbox");if(box.innerHTML){box.innerHTML="";return;}
      const r=await mapi("mail/announcement?id="+encodeURIComponent(a.id));if(!r.ok){box.innerHTML='<p class="mut">Could not load it.</p>';return;}
      const m=renderAnnouncement(r.body,{firstName:"",link:SAMPLE_LINK});
      box.innerHTML='<div class="mailgrid" style="margin-top:10px"><div>'+resultHtml(r.body)+'</div><div><div class="pvsubj">'+esc(m.subject)+'</div><iframe class="hframe" sandbox="allow-same-origin" title="What was sent"></iframe></div></div>';
      box.querySelector("iframe").srcdoc=asLight(m.html);
    };
    h.appendChild(d);
  }
}
`;

export const DESK_MAIL_STYLE = `
.mailgrid{display:grid;grid-template-columns:minmax(0,1fr) minmax(0,1fr);gap:12px;align-items:start}
@media(max-width:980px){.mailgrid{grid-template-columns:minmax(0,1fr)}}
.mailc .lab{display:block;margin:12px 0 4px}.mailc textarea#mBody{min-height:260px;line-height:1.6}
.hint{margin-top:6px}.hint code{font:12px ui-monospace,Menlo,monospace;background:#0e1118;border:1px solid var(--line);border-radius:5px;padding:1px 5px}
.aud{display:flex;gap:10px;align-items:flex-start;padding:9px 10px;border:1px solid var(--line);border-radius:10px;margin-top:6px;cursor:pointer}
.aud input,.prow input{width:auto;margin-top:3px;accent-color:var(--acc)}
.pick{margin-top:8px}.plist{max-height:260px;overflow:auto;margin-top:6px;border:1px solid var(--line);border-radius:10px}
.prow{display:flex;gap:10px;align-items:center;padding:7px 10px;border-top:1px solid var(--line);cursor:pointer}.prow:first-child{border-top:0}
.prow.off{opacity:.45;cursor:default}
button.sm{padding:4px 10px;font-size:12px}button.ghost.on{color:var(--ink);border-color:var(--mut)}
.pvsubj{font-weight:600;font-size:15px;margin:2px 0 8px}
#pvHtml,.hframe{width:100%;height:520px;border:1px solid var(--line);border-radius:10px;background:#fff}
#pvText{white-space:pre-wrap;font:12.5px/1.6 ui-monospace,Menlo,monospace;background:#0e1118;border:1px solid var(--line);border-radius:10px;padding:14px;max-height:520px;overflow:auto}
.confirm{margin-top:12px;padding:12px 14px;border:1px solid var(--acc);border-radius:10px;background:#111a2e}
button:disabled{opacity:.5;cursor:default}
`;
