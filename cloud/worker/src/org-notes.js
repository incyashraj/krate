// Release notes: a few words from the sender about what changed, read by
// whoever approves the version and by everyone who opens the release.
//
//   PUT  /orgs/<slug>/candidates/<digest>/note {note}   (0-1000 characters;
//        empty clears) -> 200 {digest, note}
//        the version's sender, or an owner; while it waits or once released
//   POST /orgs/<slug>/candidates   a note at upload: the header
//        x-krate-note, URL-encoded UTF-8, the same limit
//   GET  /orgs/<slug>/candidates, /releases and /apps   each version
//        carries `note` (null when there is none)
//
// Audited as candidate.note ("set" or "cleared"); the words themselves are
// on the version, not in the audit.
import { Org, extendOrg, addReasons } from "./org.js";
import "./org-signing.js";
import "./org-releases.js";

export const NOTE_MAX = 1000;

addReasons({
  "note-closed": {
    status: 409,
    message: (o, d) => d || "a declined or withdrawn version keeps the note it had",
  },
});

/// The x-krate-note header, decoded: `{value}` (null when absent or
/// empty), or `{bad}` with what to fix.
export function noteHeader(raw) {
  if (raw === null || raw === undefined || raw === "") return { value: null };
  let value;
  try {
    value = decodeURIComponent(String(raw)).trim();
  } catch {
    return { bad: "x-krate-note is the release note, URL-encoded (UTF-8)" };
  }
  if (value.length > NOTE_MAX) return { bad: `a release note is at most ${NOTE_MAX} characters` };
  return { value: value || null };
}

const ops = {
  "note-set"(ctx) {
    const digest = String(ctx.args.digest || "");
    const target = `candidate:${digest}`;
    const { me, refusal } = this.standing(ctx, "candidate.note", target);
    if (refusal) return refusal;
    const c = this.row("SELECT * FROM candidates WHERE digest = ?", digest);
    if (!c) return this.deny(ctx, "candidate.note", target, "digest-mismatch");
    const sender = ctx.principals.includes(c.uploaded_by);
    if (!sender && me.role !== "owner") {
      return this.deny(ctx, "candidate.note", target, "role-cannot", "only the person who sent this version, or an owner, writes its release note");
    }
    if (c.state !== "waiting-for-approval" && c.state !== "released") {
      return this.deny(ctx, "candidate.note", target, "note-closed");
    }
    const raw = ctx.args.note === null || ctx.args.note === undefined ? "" : ctx.args.note;
    if (typeof raw !== "string" || raw.trim().length > NOTE_MAX) {
      return { status: 400, body: { error: "bad-request", message: `a release note is text, at most ${NOTE_MAX} characters (empty clears it)` } };
    }
    const note = raw.trim() || null;
    if ((c.note || null) !== note) {
      this.sql.exec("UPDATE candidates SET note = ? WHERE digest = ?", note, digest);
      this.audit(me.principal, "candidate.note", target, "allowed", note ? "set" : "cleared");
    }
    return this.ok({ digest, note });
  },
};

function install(OrgClass) {
  extendOrg({ columns: [["candidates", "note", "TEXT"]], ops, mutations: ["note-set"] });
  const P = OrgClass.prototype;
  const noteOf = (store, digest) => {
    const r = store.row("SELECT note FROM candidates WHERE digest = ?", digest);
    return (r && r.note) || null;
  };

  // A note given at upload is kept with the new version.
  const add = P.op_add_candidate;
  P.op_add_candidate = function (ctx) {
    const answer = add.call(this, ctx);
    const note = typeof ctx.args.note === "string" ? ctx.args.note.trim().slice(0, NOTE_MAX) : "";
    if (answer.status === 201 && note) {
      this.sql.exec("UPDATE candidates SET note = ? WHERE digest = ? AND note IS NULL", note, String(ctx.args.digest || ""));
      if (answer.body && typeof answer.body === "object") answer.body.note = note;
    }
    return answer;
  };

  for (const name of ["op_candidates", "op_releases"]) {
    const list = P[name];
    P[name] = function (ctx) {
      const answer = list.call(this, ctx);
      if (answer.status !== 200 || !Array.isArray(answer.body)) return answer;
      answer.body = answer.body.map((r) => ({ ...r, note: noteOf(this, r.digest) }));
      return answer;
    };
  }

  // Each release in GET /apps, and anything else that shows a release.
  const view = P.releaseView;
  if (typeof view !== "function") throw new Error("org-notes: releaseView is not installed");
  P.releaseView = function (c, base, opts) {
    const out = view.call(this, c, base, opts);
    if (out && typeof out === "object") out.note = (c && c.note) || null;
    return out;
  };
}

install(Org);

const DIGEST = /^[0-9a-f]{64}$/;

/// PUT /orgs/<slug>/candidates/<digest>/note.
export async function orgNotesRoute(h) {
  const { rest, method, mutateRaw, orgAnswer } = h;
  if (rest[0] !== "candidates" || rest.length !== 3 || rest[2] !== "note" || method !== "PUT") return null;
  const digest = String(rest[1]).toLowerCase();
  if (!DIGEST.test(digest)) return orgAnswer({ status: 404, body: { error: "digest-mismatch", message: "a digest is the candidate's 64-hex sha256" } });
  return orgAnswer(await mutateRaw("note-set", (b) => ({ digest, note: b.note })));
}
