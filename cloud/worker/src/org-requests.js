// App requests: anyone in the organisation asks for an app in a sentence,
// and the people who build them answer -- planned, done, or declined, with
// a reply.
//
//   POST /orgs/<slug>/requests {text}   (5-1000 characters) -> 201 {request}
//        any active member; at most 20 open requests per person
//   GET  /orgs/<slug>/requests          -> 200 {requests: [request...]}, newest first
//        owners, admins and developers see every request; everyone else
//        sees their own
//   PUT  /orgs/<slug>/requests/<id> {state, reply}
//        state "open" | "planned" | "done" | "declined"; reply 0-1000
//        characters (empty clears); owners, admins and developers
//
//   request = {id, text, by, at, state, reply, replied_by, replied_at}
//   `by` and `replied_by` are principals ("github:alice", "email:x@y").
//
// Audited as request.create and request.update. Kept in the organisation's
// own store.
import { Org, MATRIX, extendOrg, addReasons, allows } from "./org.js";
import { sendNotices } from "./org-notify.js";

MATRIX["request.create"] = { owner: "yes", admin: "yes", developer: "yes", reviewer: "yes", recipient: "yes" };
MATRIX["request.view"] = { owner: "yes", admin: "yes", developer: "yes", reviewer: "own", recipient: "own" };
MATRIX["request.manage"] = { owner: "yes", admin: "yes", developer: "yes" };

export const REQUEST_TEXT_MIN = 5;
export const REQUEST_TEXT_MAX = 1000;
export const REPLY_MAX = 1000;
export const OPEN_PER_PERSON = 20;
export const REQUEST_STATES = ["open", "planned", "done", "declined"];
/// The most a list answers: the newest.
const LIST_MAX = 500;

addReasons({
  "no-such-request": { status: 404, message: (o, d) => d || `there is no app request with that id in ${o}` },
  "requests-full": {
    status: 409,
    message: () => `you have ${OPEN_PER_PERSON} open app requests -- wait until some are answered, then ask again`,
  },
});

const SCHEMA = [
  `CREATE TABLE IF NOT EXISTS app_requests (
     id TEXT PRIMARY KEY, text TEXT NOT NULL, asked_by TEXT NOT NULL, asked_at INTEGER NOT NULL,
     state TEXT NOT NULL, reply TEXT, replied_by TEXT, replied_at INTEGER)`,
];

function requestId() {
  const alphabet = "abcdefghjkmnpqrstuvwxyz23456789";
  const bytes = crypto.getRandomValues(new Uint8Array(12));
  return `rq_${[...bytes].map((b) => alphabet[b % alphabet.length]).join("")}`;
}

/// One request as the pages read it.
export function requestView(r) {
  return {
    id: r.id,
    text: r.text,
    by: r.asked_by,
    at: r.asked_at,
    state: r.state,
    reply: r.reply || null,
    replied_by: r.replied_by || null,
    replied_at: r.replied_at || null,
  };
}

const ops = {
  "request-create"(ctx) {
    const target = `org:${ctx.slug}`;
    const { me, refusal } = this.standing(ctx, "request.create", target);
    if (refusal) return refusal;
    if (allows(me.role, "request.create") !== "yes") {
      return this.deny(ctx, "request.create", target, "role-cannot", `a ${me.role} cannot ask ${ctx.slug} for apps`);
    }
    const text = typeof ctx.args.text === "string" ? ctx.args.text.trim() : "";
    if (text.length < REQUEST_TEXT_MIN || text.length > REQUEST_TEXT_MAX) {
      return { status: 400, body: { error: "bad-request", message: `say what app you need, in ${REQUEST_TEXT_MIN} to ${REQUEST_TEXT_MAX} characters` } };
    }
    const open = this.row("SELECT COUNT(*) AS n FROM app_requests WHERE asked_by = ? AND state = 'open'", me.principal).n;
    if (open >= OPEN_PER_PERSON) return this.deny(ctx, "request.create", target, "requests-full");
    const id = requestId();
    this.sql.exec(
      "INSERT INTO app_requests (id, text, asked_by, asked_at, state) VALUES (?, ?, ?, ?, 'open')",
      id,
      text,
      me.principal,
      ctx.now,
    );
    this.audit(me.principal, "request.create", `request:${id}`, "allowed");
    return this.ok({ request: requestView(this.row("SELECT * FROM app_requests WHERE id = ?", id)) }, 201);
  },

  "requests-list"(ctx) {
    const target = `org:${ctx.slug}`;
    const { me, refusal } = this.standing(ctx, "request.view", target);
    if (refusal) return refusal;
    const rule = allows(me.role, "request.view");
    if (rule !== "yes" && rule !== "own") {
      return this.deny(ctx, "request.view", target, "role-cannot", `a ${me.role} cannot see ${ctx.slug}'s app requests`);
    }
    const rows =
      rule === "yes"
        ? this.rows("SELECT * FROM app_requests ORDER BY asked_at DESC, rowid DESC LIMIT ?", LIST_MAX)
        : this.rows("SELECT * FROM app_requests WHERE asked_by = ? ORDER BY asked_at DESC, rowid DESC LIMIT ?", me.principal, LIST_MAX);
    return this.ok({ requests: rows.map(requestView) });
  },

  "request-update"(ctx) {
    const id = String(ctx.args.id || "");
    const target = `request:${id}`;
    const { me, refusal } = this.standing(ctx, "request.manage", target);
    if (refusal) return refusal;
    if (allows(me.role, "request.manage") !== "yes") {
      return this.deny(ctx, "request.manage", target, "role-cannot", `only ${ctx.slug}'s owners, admins and developers answer app requests`);
    }
    const r = this.row("SELECT * FROM app_requests WHERE id = ?", id);
    if (!r) return this.deny(ctx, "request.manage", target, "no-such-request");
    const state = ctx.args.state === undefined || ctx.args.state === null ? r.state : ctx.args.state;
    if (!REQUEST_STATES.includes(state)) {
      return { status: 400, body: { error: "bad-request", message: `state is one of ${REQUEST_STATES.join(", ")}` } };
    }
    let reply = r.reply || null;
    if (ctx.args.reply !== undefined && ctx.args.reply !== null) {
      if (typeof ctx.args.reply !== "string" || ctx.args.reply.trim().length > REPLY_MAX) {
        return { status: 400, body: { error: "bad-request", message: `a reply is text, at most ${REPLY_MAX} characters (empty clears it)` } };
      }
      reply = ctx.args.reply.trim() || null;
    }
    if (state !== r.state || reply !== (r.reply || null)) {
      this.sql.exec(
        "UPDATE app_requests SET state = ?, reply = ?, replied_by = ?, replied_at = ? WHERE id = ?",
        state,
        reply,
        me.principal,
        ctx.now,
        id,
      );
      this.audit(me.principal, "request.update", target, "allowed", reply !== (r.reply || null) ? `${state}, replied` : state);
    }
    return this.ok({ request: requestView(this.row("SELECT * FROM app_requests WHERE id = ?", id)) });
  },
};

extendOrg({ schema: SCHEMA, ops, mutations: ["request-create", "request-update"] });
void Org;

/// /orgs/<slug>/requests and /orgs/<slug>/requests/<id>.
export async function orgRequestsRoute(h) {
  const { rest, method, askRaw, mutateRaw, orgAnswer, later, notices } = h;
  if (rest[0] !== "requests") return null;
  if (rest.length === 1 && method === "GET") return orgAnswer(await askRaw("requests-list"));
  if (rest.length === 1 && method === "POST") {
    const answer = await mutateRaw("request-create", (b) => ({ text: b.text }));
    // The people who answer requests hear of it when they asked to.
    if (answer.status === 201 && !answer.replayed && answer.body && answer.body.request) {
      await later(sendNotices(notices, { event: "request", id: answer.body.request.id }));
    }
    return orgAnswer(answer);
  }
  if (rest.length === 2 && method === "PUT") {
    return orgAnswer(await mutateRaw("request-update", (b) => ({ id: String(rest[1]), state: b.state, reply: b.reply })));
  }
  return null;
}
