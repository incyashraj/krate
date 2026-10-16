// Approval rules and the code behind an approval (G15, G16, G22).
//
// An owner sets the organisation's approval rules; the hub enforces them
// when someone approves, and every waiting version says where it stands.
//
//   approvals            1 or 2: how many people approve a version, each
//                        someone who may approve and none its sender.
//   sensitive_approvals  1 or 2: how many for a version that asks for the
//                        network, files or the camera, or that adds any
//                        capability its current release did not have. The
//                        larger of the two applies to such a version.
//   reviewer_groups      [{app: "*" | <app id>, group}]: one of the approvers
//                        of that app (or of every app) is in that group. An
//                        app's own entry wins over "*".
//   security_group       null | <group>: when a version adds a network host
//                        its current release did not connect to, one of the
//                        approvers is in this group.
//
// An approval that does not yet meet the rules is recorded (a partial
// approval, audited as candidate.approve.partial) and the version keeps
// waiting; the approval that meets them releases it, through every check
// approving already makes, audited as candidate.approve. An approval that
// could not help meet them -- the count is met and only a group's approval
// is missing, and the approver is not in that group -- is refused
// `approval-policy`, saying whose approval is needed; approving twice is
// refused `already-approved`. Approvals are counted from current state: an
// approver who was removed, or who may no longer approve, no longer counts,
// and group membership is read when the approval is judged.
//
// Solo release (org-signing.js) is unchanged under the default rules (1
// approval, no groups): a lone owner still releases their own apps while
// nobody else may approve. Any other rules need people other than the
// sender, so they cannot be met by a lone owner: with rules set, solo
// release is off and every release needs someone else.
//
//   GET  /orgs/<slug>/settings/approvals   (any member) -> PolicyView
//   PUT  /orgs/<slug>/settings/approvals   (owner) {approvals, sensitive_approvals,
//        reviewer_groups, security_group} -> PolicyView
//        PolicyView = {policy, default, set_at, set_by, approvers: [display],
//                      warnings: [words]}
//   POST /orgs/<slug>/candidates/<digest>/approve
//        -> 202 {digest, state: "waiting-for-approval", approval} when the
//           approval is recorded and more are needed;
//           200 {release, approval} as before when it releases.
//   GET  /orgs/<slug>/candidates   each waiting row carries `approval`:
//        {rules, needed, approvals: [{principal, display, at}], groups:
//         [{group, why, met}], satisfied, words, sensitive, you: {approved,
//         can, why}}
//   GET  /orgs/<slug>/candidates/<digest>/changes   (whoever may approve,
//        and its sender) -> what changed in the code (code-changes.js),
//        against the app's current release, or as a new app. Cached in R2
//        at changes:<digest>, beside the bytes; never audited when allowed,
//        and never carrying source into the audit.
//
// GET /orgs/<slug> carries org.approval_policy (the PolicyView).

import { Org, MATRIX, extendOrg, addReasons, allows, displayOf } from "./org.js";
// Every module that wraps approving and the candidates list first.
import "./org-decline.js";
import "./org-notify.js";
import { appOf, GROUP } from "./org-releases.js";
import { changesOf } from "./org-review.js";
import { builtWords } from "./org-inventory.js";
import { codeChanges, bucketReader, CHANGES_VERSION } from "./code-changes.js";

// Who reads the code of a version: those who approve, and its sender.
MATRIX["candidate.changes"] = { owner: "yes", reviewer: "yes", developer: "own" };
// Who sets the approval rules.
MATRIX["approval.policy"] = { owner: "yes" };

export const DEFAULT_POLICY = Object.freeze({ approvals: 1, sensitive_approvals: 1, reviewer_groups: [], security_group: null });
const REVIEWER_GROUPS_MAX = 50;
const APP_MAX = 120;

addReasons({
  "approval-policy": {
    status: 403,
    message: (o, d) => d || `${o}'s approval rules need someone else's approval for this version`,
  },
  "already-approved": {
    status: 409,
    message: (o, d) => d || "you approved this version already -- it waits for the approvals the rules still need",
  },
  "policy-invalid": {
    status: 400,
    message: (o, d) => d || "approval rules name approvals (1 or 2), sensitive_approvals (1 or 2), reviewer_groups and security_group",
  },
});

const SCHEMA = [
  `CREATE TABLE IF NOT EXISTS approval_policy (
     id INTEGER PRIMARY KEY CHECK (id = 1), policy TEXT NOT NULL,
     set_at INTEGER NOT NULL, set_by TEXT NOT NULL)`,
  `CREATE TABLE IF NOT EXISTS candidate_approvals (
     digest TEXT NOT NULL, principal TEXT NOT NULL, at INTEGER NOT NULL,
     PRIMARY KEY (digest, principal))`,
];

/// "finance", "finance and security".
function spoken(list) {
  if (list.length <= 1) return list.join("");
  return `${list.slice(0, -1).join(", ")} and ${list[list.length - 1]}`;
}

function capsOf(c) {
  try {
    const parsed = JSON.parse((c && c.caps) || "[]");
    return Array.isArray(parsed) ? parsed : [];
  } catch {
    return [];
  }
}

/// Whether a capability touches the network, files or the camera.
const SENSITIVE = (cap) => /^(net|fs|camera)\./.test(String(cap || ""));
const hostOf = (cap) => (/^net\.connect:/.test(cap) ? cap.slice("net.connect:".length) : null);

/// The rules from what an owner sent, or `{problem}`.
export function policyArg(raw, groupExists) {
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return { problem: null };
  const count = (v, name) => {
    if (v === undefined || v === null) return 1;
    if (v !== 1 && v !== 2) throw new Error(`${name} is 1 or 2 approvals`);
    return v;
  };
  try {
    const approvals = count(raw.approvals, "approvals");
    const sensitive = count(raw.sensitive_approvals, "sensitive_approvals");
    const list = raw.reviewer_groups === undefined || raw.reviewer_groups === null ? [] : raw.reviewer_groups;
    if (!Array.isArray(list) || list.length > REVIEWER_GROUPS_MAX) throw new Error(`reviewer_groups is a list of at most ${REVIEWER_GROUPS_MAX} {app, group}`);
    const seen = new Set();
    const reviewer = list.map((r) => {
      const app = String((r && r.app) || "*").trim().toLowerCase().slice(0, APP_MAX) || "*";
      const group = String((r && r.group) || "").trim().toLowerCase();
      if (!GROUP.test(group)) throw new Error(`${JSON.stringify(group)} is not a group name`);
      if (!groupExists(group)) throw new Error(`there is no group called ${group} -- make it under People first`);
      if (seen.has(app)) throw new Error(app === "*" ? "only one reviewer group is for every app" : `${app} has two reviewer groups: keep one`);
      seen.add(app);
      return { app, group };
    });
    let security = raw.security_group === undefined || raw.security_group === null || raw.security_group === "" ? null : String(raw.security_group).trim().toLowerCase();
    if (security !== null) {
      if (!GROUP.test(security)) throw new Error(`${JSON.stringify(security)} is not a group name`);
      if (!groupExists(security)) throw new Error(`there is no group called ${security} -- make it under People first`);
    }
    return { policy: { approvals, sensitive_approvals: sensitive, reviewer_groups: reviewer, security_group: security } };
  } catch (e) {
    return { problem: e.message };
  }
}

export const isDefault = (p) => p.approvals === 1 && p.sensitive_approvals === 1 && !p.reviewer_groups.length && !p.security_group;

/// The rules in a sentence, for the audit and the page.
export function policyWords(p) {
  if (isDefault(p)) return "one approval from someone who approves, never its sender";
  const parts = [`${p.approvals === 1 ? "one approval" : "two approvals"} for every version`];
  if (p.sensitive_approvals > p.approvals) parts.push("two for one that asks for the network, files or the camera, or adds a capability");
  for (const r of p.reviewer_groups) parts.push(`one approver from ${r.group} for ${r.app === "*" ? "every app" : r.app}`);
  if (p.security_group) parts.push(`one from ${p.security_group} when a version adds a network host`);
  return parts.join("; ");
}

const ops = {
  approvalPolicy() {
    const r = this.row("SELECT policy, set_at, set_by FROM approval_policy WHERE id = 1");
    if (!r) return { policy: { ...DEFAULT_POLICY, reviewer_groups: [] }, set_at: null, set_by: null };
    try {
      const p = JSON.parse(r.policy);
      return { policy: { ...DEFAULT_POLICY, ...p, reviewer_groups: Array.isArray(p.reviewer_groups) ? p.reviewer_groups : [] }, set_at: r.set_at, set_by: r.set_by };
    } catch {
      return { policy: { ...DEFAULT_POLICY, reviewer_groups: [] }, set_at: r.set_at, set_by: r.set_by };
    }
  },

  policyIsDefault() {
    return isDefault(this.approvalPolicy().policy);
  },

  /// Whether `principal` may approve now: an active member whose role approves.
  approverNow(principal) {
    const m = this.member(principal);
    return Boolean(m && m.state === "active" && allows(m.role, "candidate.approve") === "yes");
  },

  /// What the rules ask of one version: how many approvals, and which
  /// groups one of its approvers must be in, each with why.
  policyFor(c) {
    const p = this.approvalPolicy().policy;
    const caps = capsOf(c);
    const base = c.state === "waiting-for-approval" ? this.replacedBy(c) : null;
    const added = changesOf(c, base).added;
    const sensitive = caps.some((x) => SENSITIVE(x.cap)) || Boolean(base && added.length);
    const needed = sensitive ? Math.max(p.approvals, p.sensitive_approvals) : p.approvals;
    const groups = [];
    const app = appOf(c);
    const own = p.reviewer_groups.find((r) => r.app === app) || p.reviewer_groups.find((r) => r.app === "*");
    if (own) groups.push({ group: own.group, why: own.app === "*" ? "every app needs one" : `every version of ${c.name || app} needs one` });
    const hosts = added.map((x) => hostOf(x.cap)).filter(Boolean);
    if (p.security_group && hosts.length) {
      const why = `it connects to ${hosts.length === 1 ? "a new host" : "new hosts"}: ${hosts.join(", ")}`;
      const same = groups.find((g) => g.group === p.security_group);
      if (same) same.why += `, and ${why}`;
      else groups.push({ group: p.security_group, why });
    }
    return { needed, groups, sensitive, rules: !isDefault(p) };
  },

  /// Where a version stands against the rules, with `extra` counted as
  /// approving too when given.
  approvalState(c, extra = null) {
    const pol = this.policyFor(c);
    const rows = this.rows("SELECT principal, at FROM candidate_approvals WHERE digest = ? ORDER BY at, rowid", c.digest);
    if (extra && !rows.some((r) => r.principal === extra)) rows.push({ principal: extra, at: this.now() });
    const counted = rows
      .filter((r) => r.principal !== c.uploaded_by && this.approverNow(r.principal))
      .map((r) => ({ principal: r.principal, display: displayOf(r.principal), at: r.at, groups: this.groupsOf(r.principal) }));
    const groups = pol.groups.map((g) => ({ ...g, met: counted.some((a) => a.groups.has(g.group)) }));
    const unmet = groups.filter((g) => !g.met);
    const satisfied = counted.length >= pol.needed && !unmet.length;
    return { ...pol, counted, groups, unmet, satisfied, words: satisfied ? null : needsWords(pol.needed, counted.length, unmet) };
  },

  /// Whether `principal`'s approval would help meet the rules.
  approvalHelps(state, principal) {
    const mine = this.groupsOf(principal);
    if (state.unmet.some((g) => mine.has(g.group))) return true;
    const left = state.needed - state.counted.length;
    return left > 0 && left - 1 >= (state.unmet.length ? 1 : 0);
  },

  /// A version's standing for the person asking, as the pages read it.
  approvalView(c, principals) {
    const s = this.approvalState(c);
    const asking = principals && principals.length ? this.actingAs(principals) : null;
    const me = asking ? this.member(asking) : null;
    let you = null;
    if (me) {
      const approved = s.counted.some((a) => a.principal === me.principal);
      const sender = principals.includes(c.uploaded_by);
      let why = null;
      if (sender) why = "you sent it, so someone else approves it";
      else if (!this.approverNow(me.principal)) why = "your role does not approve versions";
      else if (approved && !s.satisfied) why = "you approved it";
      else if (!s.satisfied && !this.approvalHelps(s, me.principal)) why = `it needs an approval from ${spoken(s.unmet.map((g) => g.group))}, and you are not in ${s.unmet.length === 1 ? "that group" : "those groups"}`;
      you = { approved, can: !why, why };
      // What this person's approval would do: release it, or leave it
      // needing what `after` says.
      if (you.can) {
        const after = this.approvalState(c, me.principal);
        you.releases = after.satisfied;
        you.after = after.words;
      }
    }
    return {
      rules: s.rules,
      needed: s.needed,
      sensitive: s.sensitive,
      approvals: s.counted.map((a) => ({ principal: a.principal, display: a.display, at: a.at })),
      groups: s.groups.map((g) => ({ group: g.group, why: g.why, met: g.met })),
      satisfied: s.satisfied,
      words: s.words,
      you,
    };
  },

  policyView() {
    const { policy, set_at, set_by } = this.approvalPolicy();
    const approvers = this.rows("SELECT principal, role FROM members WHERE state = 'active' ORDER BY added_at, rowid").filter((m) => allows(m.role, "candidate.approve") === "yes");
    const warnings = [];
    const most = Math.max(policy.approvals, policy.sensitive_approvals);
    if (!isDefault(policy) && approvers.length <= most) {
      warnings.push(
        approvers.length < most
          ? `Only ${approvers.length === 1 ? "one person" : `${approvers.length} people`} can approve, so a version that needs ${most} approvals waits until more join`
          : `Only ${approvers.length === 1 ? "one person" : `${approvers.length} people`} can approve, so a version one of them sends waits until someone else joins`,
      );
    }
    for (const g of [...new Set([...policy.reviewer_groups.map((r) => r.group), ...(policy.security_group ? [policy.security_group] : [])])]) {
      const inGroup = approvers.filter((m) => this.groupsOf(m.principal).has(g));
      if (!inGroup.length) warnings.push(`Nobody in ${g} can approve: add a reviewer or an owner to ${g}, or versions that need ${g} wait`);
    }
    return {
      policy,
      default: isDefault(policy),
      words: policyWords(policy),
      set_at,
      set_by,
      approvers: approvers.map((m) => displayOf(m.principal)),
      warnings,
    };
  },

  /// The release a version is compared with: for one waiting or declined,
  /// the app's current release; for one released, the version of the same
  /// app released before it.
  changesBase(c) {
    if (c.state === "waiting-for-approval" || c.state === "declined") return this.replacedBy(c);
    const app = appOf(c);
    const before = this.rows(
      "SELECT * FROM candidates WHERE state IN ('released', 'withdrawn') AND digest != ? AND approved_at IS NOT NULL AND approved_at <= ? ORDER BY approved_at DESC, rowid DESC",
      c.digest,
      c.approved_at || c.uploaded_at || 0,
    ).filter((x) => appOf(x) === app);
    return before[0] || null;
  },
};

/// "Needs one more approval from security".
export function needsWords(needed, have, unmet) {
  const left = Math.max(0, needed - have);
  const extra = Math.max(0, left - unmet.length);
  const more = have ? " more" : "";
  const groups = spoken(unmet.map((g) => g.group));
  if (!unmet.length) return left === 1 ? `Needs one${more} approval` : `Needs ${left}${more} approvals`;
  const fromGroups = unmet.length === 1 ? `${have ? "one more approval" : "an approval"} from ${groups}` : `approvals from ${groups}`;
  if (!extra) return `Needs ${fromGroups}`;
  return `Needs ${fromGroups} and ${extra === 1 ? "one more" : `${extra} more`} from anyone who approves`;
}

const commands = {
  "approval-policy-show"(ctx) {
    const { refusal } = this.standing(ctx, "org.view", `org:${ctx.slug}`);
    if (refusal) return refusal;
    return this.ok(this.policyView());
  },

  "approval-policy-set"(ctx) {
    const target = `org:${ctx.slug}`;
    const { me, refusal } = this.standing(ctx, "approval.policy", target);
    if (refusal) return refusal;
    if (allows(me.role, "approval.policy") !== "yes") {
      return this.deny(ctx, "approval.policy", target, "role-cannot", `only an owner sets ${this.org().name || ctx.slug}'s approval rules`);
    }
    const read = policyArg(ctx.args.policy, (g) => Boolean(this.row("SELECT name FROM groups WHERE name = ?", g)));
    if (!read.policy) return { status: 400, body: { error: "policy-invalid", message: read.problem || "approval rules name approvals (1 or 2), sensitive_approvals (1 or 2), reviewer_groups and security_group" } };
    const before = this.approvalPolicy().policy;
    if (JSON.stringify(before) !== JSON.stringify(read.policy)) {
      this.sql.exec(
        `INSERT INTO approval_policy (id, policy, set_at, set_by) VALUES (1, ?, ?, ?)
         ON CONFLICT (id) DO UPDATE SET policy = excluded.policy, set_at = excluded.set_at, set_by = excluded.set_by`,
        JSON.stringify(read.policy),
        ctx.now,
        me.principal,
      );
      this.audit(me.principal, "settings.approvals", target, "allowed", policyWords(read.policy).slice(0, 900));
    }
    return this.ok(this.policyView());
  },

  /// Who may read a version's code, and what the comparison needs from
  /// the store: the release it is compared with, its capability changes
  /// and how it was made.
  "changes-context"(ctx) {
    const digest = String(ctx.args.digest || "");
    const target = `candidate:${digest}`;
    const { me, refusal } = this.standing(ctx, "candidate.changes", target);
    if (refusal) return refusal;
    const c = this.row("SELECT * FROM candidates WHERE digest = ?", digest);
    const rule = allows(me.role, "candidate.changes");
    const sender = Boolean(c && ctx.principals.includes(c.uploaded_by));
    if (!(rule === "yes" || (rule === "own" && sender))) {
      return this.deny(ctx, "candidate.changes", target, "role-cannot", `only the people who approve versions in ${this.org().name || ctx.slug}, and whoever sent a version, read its code`);
    }
    if (!c) return this.deny(ctx, "candidate.changes", target, "digest-mismatch");
    const base = this.changesBase(c);
    const ch = changesOf(c, base);
    let prov = null;
    try {
      prov = JSON.parse(c.provenance || "null");
    } catch {
      prov = null;
    }
    return this.ok({
      digest,
      name: c.name,
      version: c.version,
      app_id: appOf(c),
      state: c.state,
      against: base ? { digest: base.digest, version: base.version || "" } : null,
      added: ch.added,
      removed: ch.removed,
      built: prov ? builtWords(prov) : null,
      ai: Boolean(prov && prov.provider),
    });
  },
};

function install(OrgClass) {
  extendOrg({
    schema: SCHEMA,
    ops: commands,
    mutations: ["approval-policy-set"],
    view(org) {
      org.approval_policy = this.policyView();
    },
  });
  const P = OrgClass.prototype;
  for (const [name, fn] of Object.entries(ops)) {
    if (Object.prototype.hasOwnProperty.call(P, name)) throw new Error(`org-approvals: ${name} already exists`);
    P[name] = fn;
  }
  for (const name of ["op_approve", "op_candidates", "op_notifications", "soloOpenFor", "twoPersonInForce", "capsRefusal", "replacedBy", "groupsOf"]) {
    if (typeof P[name] !== "function") throw new Error(`org-approvals: ${name} is not installed`);
  }

  // Rules other than the default need someone besides the sender: a lone
  // owner's solo release applies only under the default rules.
  const solo = P.soloOpenFor;
  P.soloOpenFor = function (principals) {
    if (!this.policyIsDefault()) return false;
    return solo.call(this, principals);
  };
  const two = P.twoPersonInForce;
  P.twoPersonInForce = function () {
    return !this.policyIsDefault() || two.call(this);
  };

  const approve = P.op_approve;
  P.op_approve = function (ctx) {
    const digest = String(ctx.args.digest || "");
    const c = this.row("SELECT * FROM candidates WHERE digest = ?", digest);
    if (!c || c.state !== "waiting-for-approval" || this.policyIsDefault()) return approve.call(this, ctx);
    const target = `candidate:${digest}`;
    const { me, refusal } = this.standing(ctx, "candidate.approve", target);
    if (refusal) return refusal;
    // The sender and a role that cannot approve hear the store's own words.
    if (ctx.principals.includes(c.uploaded_by) || allows(me.role, "candidate.approve") !== "yes") return approve.call(this, ctx);
    const state = this.approvalState(c);
    if (!state.satisfied) {
      const app = `${c.name || "this app"}${c.version ? ` ${c.version}` : ""}`;
      if (state.counted.some((a) => a.principal === me.principal)) {
        return this.deny(ctx, "candidate.approve", target, "already-approved", `you approved ${app} already -- it ${state.words.charAt(0).toLowerCase()}${state.words.slice(1)}`);
      }
      // What approving checks anyway, before an approval is recorded.
      const caps = this.capsRefusal(ctx, c);
      if (caps) return caps;
      if (c.signer && this.revokedKeys().has(c.signer)) return approve.call(this, ctx);
      if (!this.approvalHelps(state, me.principal)) {
        const groups = spoken(state.unmet.map((g) => g.group));
        const why = state.unmet.map((g) => g.why).join("; ");
        return this.deny(
          ctx,
          "candidate.approve",
          target,
          "approval-policy",
          `${this.org().name || ctx.slug}'s approval rules need an approval from ${groups} for ${app} (${why}), and you are not in ${state.unmet.length === 1 ? "that group" : "those groups"} -- ask someone in ${groups} to approve it`,
        );
      }
      const after = this.approvalState(c, me.principal);
      if (!after.satisfied) {
        this.sql.exec("INSERT OR IGNORE INTO candidate_approvals (digest, principal, at) VALUES (?, ?, ?)", digest, me.principal, ctx.now);
        this.audit(me.principal, "candidate.approve.partial", target, "allowed", `${after.counted.length} of ${after.needed} -- ${after.words.charAt(0).toLowerCase()}${after.words.slice(1)}`);
        return { status: 202, body: { digest, state: "waiting-for-approval", approval: this.approvalView(c, ctx.principals) } };
      }
    }
    // Met, with this approval: released through every check approving makes.
    const others = state.counted.filter((a) => a.principal !== me.principal).map((a) => a.display);
    const answer = approve.call(this, { ...ctx, approveReason: others.length ? `approval rules met, with ${others.join(", ")}` : "approval rules met" });
    if (answer.status === 200) {
      this.sql.exec("INSERT OR IGNORE INTO candidate_approvals (digest, principal, at) VALUES (?, ?, ?)", digest, me.principal, ctx.now);
      if (answer.body && typeof answer.body === "object") answer.body.approval = this.approvalView({ ...c, state: "released" }, ctx.principals);
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
      if (c.state === "waiting-for-approval") return { ...r, approval: this.approvalView(c, ctx.principals) };
      const approved = this.rows("SELECT principal FROM candidate_approvals WHERE digest = ? ORDER BY at, rowid", c.digest).map((x) => x.principal);
      return approved.length > 1 ? { ...r, approved_with: approved } : r;
    });
    return answer;
  };

  // A version someone already approved, or cannot help approve, is not
  // waiting for them.
  const notes = P.op_notifications;
  P.op_notifications = function (ctx) {
    const answer = notes.call(this, ctx);
    if (answer.status !== 200 || !answer.body || !Array.isArray(answer.body.items) || this.policyIsDefault()) return answer;
    const me = this.member(ctx.actor);
    if (!me) return answer;
    answer.body.items = answer.body.items.filter((i) => {
      if (i.kind !== "approval-waiting") return true;
      const c = this.row("SELECT * FROM candidates WHERE digest = ?", i.target);
      if (!c) return true;
      const v = this.approvalView(c, ctx.principals);
      return !v.you || v.you.can;
    });
    return answer;
  };
}

install(Org);

// ---------------------------------------------------------------- routes

const DIGEST = /^[0-9a-f]{64}$/;

async function readerFor(env, digest) {
  const direct = await bucketReader(env.BUNDLES, digest);
  if (direct) return direct;
  const blob = await env.APPS.get(`bigkey:${digest}`).catch(() => null);
  return blob ? bucketReader(env.BUNDLES, blob) : null;
}

/// Where a version's code comparison is kept: beside its bytes, removed
/// with them when a closed organisation's files are deleted.
export const changesKey = (digest) => `changes:${digest}`;

/// The code comparison for one version, from the cache or made now.
export async function changesFor(env, slug, facts) {
  const against = facts.against ? facts.against.digest : null;
  const key = changesKey(facts.digest);
  try {
    const cached = await env.BUNDLES.get(key);
    if (cached) {
      const kept = JSON.parse(new TextDecoder().decode(await cached.arrayBuffer()));
      if (kept && kept.v === CHANGES_VERSION && kept.against === against && kept.result) return { result: kept.result, cached: true };
    }
  } catch (_) {}
  const next = await readerFor(env, facts.digest);
  if (!next) return { missing: true };
  const prev = against ? await readerFor(env, against) : null;
  const result = await codeChanges(next, prev, facts);
  if (against && !prev) result.risks.unshift({ kind: "no-source", level: "note", text: "The release it replaces could not be read here, so every file shows as new" });
  try {
    const body = new TextEncoder().encode(JSON.stringify({ v: CHANGES_VERSION, against, result }));
    await env.BUNDLES.put(key, body, { httpMetadata: { contentType: "application/json" }, customMetadata: { org: slug } });
  } catch (_) {}
  return { result, cached: false };
}

/// GET and PUT /orgs/<slug>/settings/approvals; GET
/// /orgs/<slug>/candidates/<digest>/changes.
export async function orgApprovalsRoute(h) {
  const { env, slug, rest, method, askRaw, mutateRaw, orgAnswer } = h;
  if (rest[0] === "settings" && rest[1] === "approvals" && rest.length === 2) {
    if (method === "GET") return orgAnswer(await askRaw("approval-policy-show"));
    if (method === "PUT") return orgAnswer(await mutateRaw("approval-policy-set", (b) => ({ policy: b })));
    return null;
  }
  if (rest[0] === "candidates" && rest.length === 3 && rest[2] === "changes" && method === "GET") {
    const digest = String(rest[1]).toLowerCase();
    if (!DIGEST.test(digest)) return orgAnswer({ status: 404, body: { error: "digest-mismatch", message: "a digest is the candidate's 64-hex sha256" } });
    const facts = await askRaw("changes-context", { digest });
    if (facts.status !== 200) return orgAnswer(facts);
    const made = await changesFor(env, slug, facts.body);
    if (made.missing) {
      return orgAnswer({ status: 404, body: { error: "digest-mismatch", message: "the hub does not hold this version's bytes any more -- its sender sends it again" } });
    }
    const f = facts.body;
    const res = orgAnswer({ status: 200, body: { digest, name: f.name, version: f.version, app_id: f.app_id, state: f.state, against: f.against, ...made.result } });
    res.headers.set("cache-control", "private, no-store");
    return res;
  }
  return null;
}
