// What a reviewer approves (IC-950): an approval is of what an app can do,
// not of a name and a hash.
//
// Each candidate keeps the capabilities its manifest asks for, in the plain
// words the validator gives them (`Capability::plain`, the same words the
// consent prompt shows). The candidates list carries them and what changed
// against the app's current release: added, removed, kept. Approving a
// release that adds something a person is asked for (network, files, the
// camera...) needs the approver to name each addition: an approval that
// silently widens what an app can touch is the hole this closes (ch 13,
// step 3: a change "cannot hide a capability change").
//
// A first release has nothing to compare with; its whole list is shown
// and the approval is of that list. The check runs only after the usual
// refusals (not a member, approving your own upload, a role that cannot
// approve), so nobody hears about capabilities they may not act on.

import { Org, extendOrg, addReasons, allows } from "./org.js";
// Release control decides what "current" is; it must be installed first.
import { appOf } from "./org-releases.js";

const CAPS_MAX = 200;
const TEXT_MAX = 300;

addReasons({
  "new-capabilities": {
    status: 409,
    message: (o, d) => d || "this release asks for more than the one it replaces: confirm each new capability to approve it",
  },
});

const COLUMNS = [["candidates", "caps", "TEXT"]];

const clip = (v) => String(v == null ? "" : v).slice(0, TEXT_MAX);

/// The capabilities a candidate carries, bounded, from the upload's
/// arguments (the validator's judgement).
export function capsArg(raw) {
  if (!Array.isArray(raw)) return [];
  return raw.slice(0, CAPS_MAX).map((c) => ({
    cap: clip(c && c.cap),
    words: clip((c && c.words) || (c && c.cap)),
    required: Boolean(c && c.required),
    rationale: clip(c && c.rationale),
    // Unknown means asked: never assume something is harmless.
    asks: c && c.asks === false ? false : true,
  })).filter((c) => c.cap);
}

/// The capability list of a judgement, from the validator's own field, or
/// for a validator from before it, from the manifest (every one treated as
/// asked).
export function judgementCaps(judgement) {
  if (judgement && Array.isArray(judgement.capabilities)) return capsArg(judgement.capabilities);
  const requests = (judgement && judgement.manifest && judgement.manifest.capabilities) || [];
  return capsArg(requests.map((r) => ({ cap: r.cap, words: r.cap, required: r.required, rationale: r.rationale })));
}

function capsOf(c) {
  try {
    const parsed = JSON.parse((c && c.caps) || "[]");
    return Array.isArray(parsed) ? parsed : [];
  } catch {
    return [];
  }
}

/// What `c` changes against `base` (another candidate, or null for a first
/// release): added and removed capabilities, and how many are kept.
export function changesOf(c, base) {
  const mine = capsOf(c);
  if (!base) return { against: null, added: mine, removed: [], kept: 0 };
  const theirs = capsOf(base);
  const had = new Set(theirs.map((x) => x.cap));
  const has = new Set(mine.map((x) => x.cap));
  return {
    against: { digest: base.digest, version: base.version || "" },
    added: mine.filter((x) => !had.has(x.cap)),
    removed: theirs.filter((x) => !has.has(x.cap)),
    kept: mine.filter((x) => had.has(x.cap)).length,
  };
}

function install(OrgClass) {
  extendOrg({ columns: COLUMNS });
  const P = OrgClass.prototype;

  /// The release `c` would replace: the app's current one, unless that is
  /// `c` itself.
  P.replacedBy = function (c) {
    const digest = this.currentOf(appOf(c));
    if (!digest || digest === c.digest) return null;
    return this.row("SELECT * FROM candidates WHERE digest = ?", digest);
  };

  const add = P.op_add_candidate;
  P.op_add_candidate = function (ctx) {
    const answer = add.call(this, ctx);
    if (answer.status === 201) {
      this.sql.exec(
        "UPDATE candidates SET caps = ? WHERE digest = ? AND caps IS NULL",
        JSON.stringify(capsArg(ctx.args.capabilities)),
        String(ctx.args.digest || ""),
      );
    }
    return answer;
  };

  const list = P.op_candidates;
  P.op_candidates = function (ctx) {
    const answer = list.call(this, ctx);
    if (answer.status !== 200 || !Array.isArray(answer.body)) return answer;
    answer.body = answer.body.map((r) => {
      const c = this.row("SELECT * FROM candidates WHERE digest = ?", r.digest);
      if (!c) return r;
      return {
        ...r,
        signer: c.signer || null,
        capabilities: capsOf(c),
        // Only a waiting candidate is a change someone is deciding on.
        changes: c.state === "waiting-for-approval" ? changesOf(c, this.replacedBy(c)) : null,
      };
    });
    return answer;
  };

  /// The refusal for approving `c` without naming each capability it adds
  /// that a person is asked for, or null. Approval rules (org-approvals.js)
  /// ask it too, before they record an approval that does not release.
  P.capsRefusal = function (ctx, c) {
    const base = this.replacedBy(c);
    if (!base) return null;
    const confirmed = new Set(Array.isArray(ctx.args.accept) ? ctx.args.accept.map(String) : []);
    const unconfirmed = changesOf(c, base).added.filter((x) => x.asks && !confirmed.has(x.cap));
    if (!unconfirmed.length) return null;
    const refusal = this.deny(
      ctx,
      "candidate.approve",
      `candidate:${c.digest}`,
      "new-capabilities",
      `${c.name || "this app"} ${c.version || ""} asks for more than ${base.version || "the release it replaces"}: ${unconfirmed
        .map((x) => x.words)
        .join("; ")} -- confirm each to approve it`.replace(/ {2,}/g, " "),
    );
    refusal.body.added = unconfirmed;
    return refusal;
  };

  const approve = P.op_approve;
  P.op_approve = function (ctx) {
    const digest = String(ctx.args.digest || "");
    const c = this.row("SELECT * FROM candidates WHERE digest = ?", digest);
    const me = ctx.actor ? this.member(ctx.actor) : null;
    // Every other refusal first, in the words the store already gives.
    const decides =
      c &&
      c.state === "waiting-for-approval" &&
      me &&
      me.state === "active" &&
      !ctx.principals.includes(c.uploaded_by) &&
      allows(me.role, "candidate.approve") === "yes";
    if (decides) {
      const refusal = this.capsRefusal(ctx, c);
      if (refusal) return refusal;
    }
    return approve.call(this, ctx);
  };
}

install(Org);
