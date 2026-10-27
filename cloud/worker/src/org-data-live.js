// Team data that feels live: push instead of poll (SMART-FIXES 7).
//
// A device or a tab that is syncing an organisation's app (org-data.js)
// also holds one WebSocket to the organisation's Durable Object. When a
// sync writes something new for that app, the object sends every other
// socket of that app one tiny message -- {"t":"changed","s":"<cursor>"} --
// and those devices sync at once. The message names no record, no field,
// no person and no app (the socket is already one app's): nothing secret
// travels on it, and the data itself still only moves sealed, by sync.
//
// Who may hold a socket is exactly who may sync: a socket is opened with a
// one-use ticket that a sync hands out (`live: true` in the sync's body),
// so the sync's checks -- standing, a release of this organisation, not
// withdrawn, inside its audience, the role's data.use -- are the ones that
// let it in. The ticket goes in the Sec-WebSocket-Protocol header, never
// the URL, is kept here only as its hash, and runs out in a minute.
//
// The standing is asked again:
// - when the socket is opened (the ticket could be up to a minute old);
// - after every command that could change it (a removal, a role, a
//   withdrawal, an audience, SCIM, a close), and a socket whose person or
//   release no longer passes is closed (code 4001);
// - before each "changed" is sent, so a socket that slipped past the two
//   above still never hears anything after the person lost access.
// Those checks are quiet: they are not attempts by anyone, so they add
// nothing to the audit.
//
// Bounded: at most LIVE_PER_PERSON_APP sockets per person per app (a new
// one closes the oldest, so a tab reconnecting in a loop cannot pile them
// up), and LIVE_PER_ORG in all.
//
// The hibernation API (state.acceptWebSocket): an idle socket costs no
// duration, and the object wakes only for a command, an upgrade or a
// message. Clients ping with the text "ping"; the auto-response answers
// "pong" without waking the object at all.

import { Org, extendOrg, sha256HexSync } from "./org.js";
import "./org-data.js";

/// A ticket's life, in seconds.
export const TICKET_SECONDS = 60;
/// The most sockets one person holds for one app (laptop, phone, tabs).
export const LIVE_PER_PERSON_APP = 8;
/// The most sockets one organisation holds in all.
export const LIVE_PER_ORG = 10000;
/// The subprotocol a live socket speaks; the ticket rides beside it.
export const LIVE_PROTOCOL = "krate-live";
/// Closed because the person or the release no longer passes.
export const CLOSE_STANDING = 4001;
/// Closed to make room for the same person's newer socket.
export const CLOSE_REPLACED = 4002;

/// Commands that never change anyone's standing and run on every sync:
/// no re-check after them.
const HOT = new Set(["data-sync", "data-usage", "keys-view", "show", "exists", "invite-door"]);

const SCHEMA = [
  `CREATE TABLE IF NOT EXISTS org_data_live_tickets (
     hash TEXT PRIMARY KEY, principal TEXT NOT NULL, digest TEXT NOT NULL,
     app_id TEXT NOT NULL, device TEXT NOT NULL, expires_at INTEGER NOT NULL)`,
];

const HEX64 = /^[0-9a-f]{64}$/;
const tagOf = (kind, value) => `${kind}:${sha256HexSync(value).slice(0, 32)}`;

function randomHex(n) {
  const bytes = crypto.getRandomValues(new Uint8Array(n));
  return [...bytes].map((b) => b.toString(16).padStart(2, "0")).join("");
}

const ops = {
  /// The sockets this object holds, or none where the platform has no
  /// hibernation API (a test that does not ask for it).
  liveSockets(tag) {
    if (!this.ctx || typeof this.ctx.getWebSockets !== "function") return [];
    return tag === undefined ? this.ctx.getWebSockets() : this.ctx.getWebSockets(tag);
  },

  /// A ticket for `me`'s socket to `app`, inside the sync's transaction.
  liveTicket(me, app, device, ctx) {
    const now = ctx.now;
    this.sql.exec("DELETE FROM org_data_live_tickets WHERE expires_at <= ?", now);
    // A device asking in a loop keeps a handful, never a pile.
    const mine = this.rows(
      "SELECT hash FROM org_data_live_tickets WHERE principal = ? AND app_id = ? ORDER BY expires_at DESC",
      me.principal,
      app,
    );
    for (const old of mine.slice(LIVE_PER_PERSON_APP - 1)) this.sql.exec("DELETE FROM org_data_live_tickets WHERE hash = ?", old.hash);
    const ticket = randomHex(32);
    this.sql.exec(
      "INSERT INTO org_data_live_tickets (hash, principal, digest, app_id, device, expires_at) VALUES (?, ?, ?, ?, ?, ?)",
      sha256HexSync(ticket),
      me.principal,
      String(ctx.args.digest || ""),
      app,
      String(device || ""),
      now + TICKET_SECONDS,
    );
    return { ticket, protocol: LIVE_PROTOCOL, expires_in: TICKET_SECONDS };
  },

  /// Whether the socket's person may still sync this release: the sync's
  /// own checks, asked quietly. Null when they may, else the refusal.
  liveStanding(att, now) {
    const o = this.org();
    if (!o) return { status: 404, body: { error: "no-such-org" } };
    const ctx = {
      op: "data-sync",
      slug: o.slug,
      actor: att.p,
      principals: [att.p],
      args: { digest: att.d },
      now,
      quiet: true,
    };
    const gated = this.gateRefusal(ctx);
    if (gated) return gated;
    const { app, refusal } = this.dataStanding(ctx, "data.sync");
    if (refusal) return refusal;
    if (app !== att.a) return { status: 403, body: { error: "not-released" } };
    return null;
  },

  /// Redeem a ticket (one use), inside a transaction. Answers what the
  /// socket carries, or the refusal.
  liveRedeem(digest, ticket, now) {
    if (!this.org()) return { refusal: { status: 404, message: "there is no such organisation" } };
    const hash = sha256HexSync(String(ticket || ""));
    const t = this.row("SELECT * FROM org_data_live_tickets WHERE hash = ?", hash);
    if (t) this.sql.exec("DELETE FROM org_data_live_tickets WHERE hash = ?", hash);
    if (!t || t.expires_at <= now || t.digest !== digest) {
      return { refusal: { status: 401, message: "that live ticket is not one, was used, or ran out -- sync again for a new one" } };
    }
    const att = { p: t.principal, d: t.digest, a: t.app_id, dev: t.device, at: Date.now() };
    const refusal = this.liveStanding(att, now);
    if (refusal) return { refusal: { status: refusal.status || 403, message: (refusal.body && refusal.body.message) || "not allowed" } };
    if (this.liveSockets().length >= LIVE_PER_ORG) return { refusal: { status: 503, message: "too many live connections -- syncing every few seconds instead" } };
    return { att, tags: [tagOf("a", att.a), tagOf("p", att.p), tagOf("pa", `${att.p}\u0000${att.a}`)] };
  },

  /// Close the oldest of a person's sockets for an app beyond the bound.
  liveBound(att) {
    const theirs = this.liveSockets(tagOf("pa", `${att.p}\u0000${att.a}`))
      .map((ws) => ({ ws, at: (ws.deserializeAttachment() || {}).at || 0 }))
      .sort((x, y) => x.at - y.at);
    for (const { ws } of theirs.slice(0, Math.max(0, theirs.length - LIVE_PER_PERSON_APP))) {
      try {
        ws.close(CLOSE_REPLACED, "a newer connection took its place");
      } catch (_) {}
    }
  },

  /// The upgrade, from the Worker (orgLiveRoute).
  liveUpgrade(request) {
    const url = new URL(request.url);
    const parts = url.pathname.split("/").filter(Boolean);
    const digest = String(parts[3] || "").toLowerCase();
    const offered = String(request.headers.get("sec-websocket-protocol") || "").split(",").map((s) => s.trim());
    const ticket = offered.find((p) => HEX64.test(p));
    if (!offered.includes(LIVE_PROTOCOL) || !ticket || !HEX64.test(digest)) {
      return new Response("a live socket offers the krate-live protocol and a ticket from a sync", { status: 400 });
    }
    const now = this.now();
    const got = this.ctx.storage.transactionSync(() => this.liveRedeem(digest, ticket, now));
    if (got.refusal) return new Response(got.refusal.message, { status: got.refusal.status });
    const [client, server] = Object.values(new WebSocketPair());
    this.ctx.acceptWebSocket(server, got.tags);
    server.serializeAttachment(got.att);
    this.liveBound(got.att);
    return this.liveResponse(client);
  },

  /// The 101 the Worker hands back. A method so a test outside Workers
  /// (where a Response cannot be 101) can take the client end instead.
  liveResponse(client) {
    return new Response(null, { status: 101, webSocket: client, headers: { "sec-websocket-protocol": LIVE_PROTOCOL } });
  },

  /// Close every socket whose person or release no longer passes.
  liveRecheck() {
    const now = this.now();
    const seen = new Map();
    let closed = 0;
    for (const ws of this.liveSockets()) {
      const att = ws.deserializeAttachment();
      if (!att) continue;
      const key = `${att.p}\u0000${att.d}\u0000${att.a}`;
      if (!seen.has(key)) seen.set(key, this.liveStanding(att, now));
      if (seen.get(key)) {
        closed += 1;
        try {
          ws.close(CLOSE_STANDING, "no longer allowed to use this app's data");
        } catch (_) {}
      }
    }
    return closed;
  },

  /// Tell the app's other sockets something new is there: the cursor only.
  liveChanged(note) {
    const now = this.now();
    const seen = new Map();
    const message = JSON.stringify({ t: "changed", s: String(note.cursor) });
    let sent = 0;
    for (const ws of this.liveSockets(tagOf("a", note.app))) {
      const att = ws.deserializeAttachment();
      if (!att || att.a !== note.app) continue;
      // The device that wrote it already has it.
      if (note.device && att.dev === note.device) continue;
      const key = `${att.p}\u0000${att.d}`;
      if (!seen.has(key)) seen.set(key, this.liveStanding(att, now));
      try {
        if (seen.get(key)) ws.close(CLOSE_STANDING, "no longer allowed to use this app's data");
        else {
          ws.send(message);
          sent += 1;
        }
      } catch (_) {
        // A socket going away is the client's to notice.
      }
    }
    return sent;
  },
};

function install(OrgClass) {
  Object.assign(OrgClass.prototype, ops);
  extendOrg({ schema: SCHEMA });
  const P = OrgClass.prototype;
  for (const name of ["dataStanding", "gateRefusal", "op_data_sync"]) {
    if (typeof P[name] !== "function") throw new Error(`org-data-live: ${name} is not installed`);
  }

  // Which command ran, and how it answered, for after its transaction.
  const run = P.run;
  P.run = function (command) {
    const answer = run.call(this, command);
    this.liveLast = { op: command && command.op, status: answer && answer.status };
    return answer;
  };

  const fetch = P.fetch;
  P.fetch = async function (request) {
    // The ping answer, once per wake: it answers without waking us again.
    if (!this.liveArmed) {
      armAutoResponse(this.ctx);
      this.liveArmed = true;
    }
    if ((request.headers.get("upgrade") || "").toLowerCase() === "websocket") return this.liveUpgrade(request);
    this.dataChanged = null;
    this.liveLast = null;
    const res = await fetch.call(this, request);
    // After the commit, never inside it: a rolled-back sync tells no one.
    try {
      const note = this.dataChanged;
      this.dataChanged = null;
      if (note) this.liveChanged(note);
      const last = this.liveLast;
      if (last && !HOT.has(last.op) && last.status >= 200 && last.status < 300 && this.liveSockets().length) this.liveRecheck();
    } catch (err) {
      console.error("live team data:", err && err.message);
    }
    return res;
  };

}

/// The object's hibernation handlers: a client says nothing but "ping"
/// (answered by the auto-response); anything else is ignored.
Org.prototype.webSocketMessage = async function () {};
Org.prototype.webSocketClose = async function (ws, code) {
  try {
    ws.close(code === 1005 || code === 1006 ? 1000 : code, "closed");
  } catch (_) {}
};
Org.prototype.webSocketError = async function () {};

install(Org);

/// Set the "ping" -> "pong" auto-response once per wake (the constructor
/// runs on each), where the platform has it.
export function armAutoResponse(ctx) {
  try {
    if (ctx && typeof ctx.setWebSocketAutoResponse === "function" && typeof WebSocketRequestResponsePair === "function") {
      ctx.setWebSocketAutoResponse(new WebSocketRequestResponsePair("ping", "pong"));
    }
  } catch (_) {}
}

// ---------------------------------------------------------------- routes

const SLUG = /^[a-z][a-z0-9-]{1,38}$/;

/// GET /orgs/<slug>/data/<digest>/live with Upgrade: websocket. Answered
/// before CORS and the session check: the ticket is the proof, and the
/// object judges it. Bounded per caller like other requests that carry no
/// session. Answers a Response, or null when the path is not this one.
export async function orgLiveRoute(request, url, env, { rateLimited }) {
  const parts = url.pathname.split("/").filter(Boolean);
  if (parts.length !== 5 || parts[0] !== "orgs" || parts[2] !== "data" || parts[4] !== "live") return null;
  if (request.method !== "GET") return new Response("a live socket is a GET", { status: 405 });
  const slug = String(parts[1]).toLowerCase();
  if (!SLUG.test(slug) || !HEX64.test(String(parts[3]).toLowerCase())) return new Response("not found", { status: 404 });
  const offered = String(request.headers.get("sec-websocket-protocol") || "").split(",").map((s) => s.trim());
  if (!offered.includes(LIVE_PROTOCOL) || !offered.some((p) => HEX64.test(p))) {
    return new Response("a live socket offers the krate-live protocol and a ticket from a sync", { status: 400 });
  }
  if (!env.ORGS) return new Response("organisations are not switched on on this hub", { status: 503 });
  const over = await rateLimited(request, env, "org/live");
  if (over) return over;
  return env.ORGS.get(env.ORGS.idFromName(`org:${slug}`)).fetch(request);
}
