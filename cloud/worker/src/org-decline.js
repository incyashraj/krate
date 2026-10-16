// Declining a version, with a reason the sender reads.
//
// Until this, a version nobody wanted waited for ever: the only answer an
// approver had was to approve it. Now whoever may approve a version may
// decline it instead, saying why in a sentence ("it asks for the camera").
// The same separation as approving: whoever sent it never decides on it,
// whatever their role. A declined version stays declined -- approving it
// later is refused -- and the sender sends a new one.
//
//   POST /orgs/<slug>/candidates/<digest>/decline {reason}   (1-500 characters)
//        -> 200 {digest, state: "declined", declined_by, declined_at, decline_reason}
//   GET  /orgs/<slug>/candidates   each row carries declined_by, declined_at
//        and decline_reason (null unless declined)
//
// Audited as candidate.decline, with the reason. The sender is mailed when
// their preferences allow (org-notify.js).
import { Org, extendOrg, addReasons, allows } from "./org.js";
// Every module that wraps approving is installed first: this one's guard
// on a declined version is the outermost.
import "./org-signing.js";
import "./org-inventory.js";
import { sendNotices } from "./org-notify.js";

export const DECLINE_REASON_MAX = 500;

addReasons({
  declined: {
    status: 409,
    message: (o, d) => d || "this version was declined -- its sender sends a new version instead",
  },
  "not-waiting": {
    status: 409,
    message: (o, d) => d || "only a version that waits for approval can be declined",
  },
});

const ops = {
  decline(ctx) {
    const digest = String(ctx.args.digest || "");
    const target = `candidate:${digest}`;
    const { me, refusal } = this.standing(ctx, "candidate.decline", target);
    if (refusal) return refusal;
    const c = this.row("SELECT * FROM candidates WHERE digest = ?", digest);
    // Separation first, as approving: the sender hears someone else decides.
    if (c && ctx.principals.includes(c.uploaded_by)) {
      return this.deny(ctx, "candidate.decline", target, "self-approval", "you sent this version, so someone else who approves decides on it");
    }
    if (allows(me.role, "candidate.approve") !== "yes") {
      return this.deny(ctx, "candidate.decline", target, "role-cannot", `a ${me.role} cannot decline versions in ${ctx.slug}`);
    }
    if (!c) return this.deny(ctx, "candidate.decline", target, "digest-mismatch");
    if (c.state === "declined") return this.deny(ctx, "candidate.decline", target, "declined", "this version was already declined");
    if (c.state !== "waiting-for-approval") {
      return this.deny(ctx, "candidate.decline", target, "not-waiting", `this version is ${c.state === "released" ? "already released" : c.state} -- only a version that waits can be declined`);
    }
    const reason = typeof ctx.args.reason === "string" ? ctx.args.reason.trim() : "";
    if (!reason || reason.length > DECLINE_REASON_MAX) {
      return { status: 400, body: { error: "bad-request", message: `say why it is declined, in 1 to ${DECLINE_REASON_MAX} characters: its sender reads it` } };
    }
    this.sql.exec(
      "UPDATE candidates SET state = 'declined', declined_by = ?, declined_at = ?, decline_reason = ? WHERE digest = ?",
      me.principal,
      ctx.now,
      reason,
      digest,
    );
    this.audit(me.principal, "candidate.decline", target, "allowed", reason);
    return this.ok({ digest, state: "declined", declined_by: me.principal, declined_at: ctx.now, decline_reason: reason });
  },
};

function install(OrgClass) {
  extendOrg({
    columns: [
      ["candidates", "declined_by", "TEXT"],
      ["candidates", "declined_at", "INTEGER"],
      ["candidates", "decline_reason", "TEXT"],
    ],
    ops,
    mutations: ["decline"],
  });
  const P = OrgClass.prototype;

  // A declined version is never released: approving it is refused, by
  // anyone, before anything else about approving is asked.
  const approve = P.op_approve;
  P.op_approve = function (ctx) {
    const digest = String(ctx.args.digest || "");
    const target = `candidate:${digest}`;
    const c = this.row("SELECT state FROM candidates WHERE digest = ?", digest);
    if (c && c.state === "declined") {
      const { refusal } = this.standing(ctx, "candidate.approve", target);
      if (refusal) return refusal;
      return this.deny(ctx, "candidate.approve", target, "declined");
    }
    return approve.call(this, ctx);
  };

  const list = P.op_candidates;
  P.op_candidates = function (ctx) {
    const answer = list.call(this, ctx);
    if (answer.status !== 200 || !Array.isArray(answer.body)) return answer;
    answer.body = answer.body.map((r) => {
      const c = this.row("SELECT declined_by, declined_at, decline_reason FROM candidates WHERE digest = ?", r.digest);
      return { ...r, declined_by: (c && c.declined_by) || null, declined_at: (c && c.declined_at) || null, decline_reason: (c && c.decline_reason) || null };
    });
    return answer;
  };
}

install(Org);

const DIGEST = /^[0-9a-f]{64}$/;

/// POST /orgs/<slug>/candidates/<digest>/decline.
export async function orgDeclineRoute(h) {
  const { rest, method, mutateRaw, orgAnswer, later, notices } = h;
  if (rest[0] !== "candidates" || rest.length !== 3 || rest[2] !== "decline" || method !== "POST") return null;
  const digest = String(rest[1]).toLowerCase();
  if (!DIGEST.test(digest)) return orgAnswer({ status: 404, body: { error: "digest-mismatch", message: "a digest is the candidate's 64-hex sha256" } });
  const answer = await mutateRaw("decline", (b) => ({ digest, reason: b.reason }));
  // Its sender hears why; never holds up or fails the decline.
  if (answer.status === 200 && !answer.replayed) await later(sendNotices(notices, { event: "declined", digest }));
  return orgAnswer(answer);
}
