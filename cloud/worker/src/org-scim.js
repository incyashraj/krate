// SCIM 2.0 provisioning for an organisation (IC-955; RFC 7643 and 7644).
//
// A company's identity provider (Okta, Microsoft Entra, or any SCIM 2.0
// client) keeps the organisation's people and groups in step with its own
// directory, at https://hub.krate.tech/scim/v2/<slug>/, with a bearer token
// an owner makes in Settings. The token is shown once and stored only as its
// SHA-256; revoking it stops it at the next request.
//
// What the directory's records become here:
//
// - A User is a member whose principal is `email:<userName>`. `userName` is
//   the person's email address: Krate names people by the inbox they prove.
//   A person the directory creates joins at once, with no invitation, as the
//   role the owner chose as the SCIM default (recipient unless set). Never
//   owner or admin: authority over an organisation is given by a person in
//   Krate, never by a directory (the same rule as company sign-in's
//   auto-join).
// - `active: false`, or DELETE, removes them exactly as an owner removing
//   them would: their next check-in, download and lease are refused as
//   `removed`, their open invitations go, they leave every group. Audited
//   with actor `scim`. One rule stays above the directory: the last owner
//   is never removed (`last-owner`).
// - The directory brings back only people it took away. Someone an owner
//   removed in Krate stays removed whatever the directory says; IT sees
//   the refusal in its provisioning log, in words.
// - A Group is one of the organisation's groups (IC-949 R, org-releases.js):
//   `Finance Team` becomes the group `finance-team`, or links to a group of
//   that name an owner already made. Its members are the directory's.
//   Renaming it in the directory renames only what is shown: the group's
//   name is what release audiences name, so it never changes underneath
//   them.
//
// Two halves, as in org-sso.js: commands for the Org Durable Object (one
// transaction each, like every other command), and the Worker's routes,
// which parse SCIM's request shapes, hash the token, and answer in SCIM's
// shapes (application/scim+json, the RFC 7644 error schema).

import { Org, extendOrg, allows, parsePrincipal, displayOf, sha256HexSync } from "./org.js";
import { lifecycle, billingState, seatsUsed, freeSeats } from "./org-lifecycle-store.js";
import { GROUP } from "./org-releases.js";

/// The roles the directory may give. Never owner or admin.
export const SCIM_ROLES = ["developer", "reviewer", "recipient"];
export const SCIM_DEFAULT_ROLE = "recipient";
const SEAT_ROLES = new Set(["owner", "admin", "developer", "reviewer"]);

const USER_SCHEMA = "urn:ietf:params:scim:schemas:core:2.0:User";
const GROUP_SCHEMA = "urn:ietf:params:scim:schemas:core:2.0:Group";
const LIST_SCHEMA = "urn:ietf:params:scim:api:messages:2.0:ListResponse";
const ERROR_SCHEMA = "urn:ietf:params:scim:api:messages:2.0:Error";
const PATCH_SCHEMA = "urn:ietf:params:scim:api:messages:2.0:PatchOp";
export const TOKEN_PREFIX = "kscim_";
const NAME_MAX = 120;
const MAX_COUNT = 200;
const MAX_MEMBERS = 1000;

const SCHEMA = [
  `CREATE TABLE IF NOT EXISTS scim_tokens (
     id TEXT PRIMARY KEY, hash TEXT NOT NULL UNIQUE, hint TEXT NOT NULL,
     created_at INTEGER NOT NULL, created_by TEXT NOT NULL,
     last_used_at INTEGER, revoked_at INTEGER, revoked_by TEXT)`,
  `CREATE TABLE IF NOT EXISTS scim_settings (
     id INTEGER PRIMARY KEY CHECK (id = 1),
     default_role TEXT NOT NULL, set_at INTEGER NOT NULL, set_by TEXT NOT NULL)`,
  `CREATE TABLE IF NOT EXISTS scim_users (
     principal TEXT PRIMARY KEY, external_id TEXT, given_name TEXT, family_name TEXT,
     display_name TEXT, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
     deleted INTEGER NOT NULL DEFAULT 0)`,
  `CREATE TABLE IF NOT EXISTS scim_groups (
     name TEXT PRIMARY KEY, display_name TEXT NOT NULL, external_id TEXT,
     created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL)`,
];

// ------------------------------------------------------------- helpers

const iso = (seconds) => new Date((seconds || 0) * 1000).toISOString().replace(/\.\d{3}Z$/, "Z");
const clip = (v, n = NAME_MAX) => (v === undefined || v === null ? null : String(v).slice(0, n));

/// A User's id: stable for as long as the organisation knows the address,
/// and nothing a reader can turn back into it without the slug.
export function userIdOf(slug, principal) {
  return `u${sha256HexSync(`krate-scim-user:${slug}:${principal}`).slice(0, 24)}`;
}
export function groupIdOf(slug, name) {
  return `g${sha256HexSync(`krate-scim-group:${slug}:${name}`).slice(0, 24)}`;
}

/// The organisation group a directory group's name becomes: "Finance Team"
/// is `finance-team`. Null when nothing usable is left.
export function groupNameOf(displayName) {
  let s = String(displayName || "")
    .toLowerCase()
    .normalize("NFKD")
    .replace(/[̀-ͯ]/g, "")
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
  if (s && !/^[a-z]/.test(s)) s = `g-${s}`;
  s = s.slice(0, 39).replace(/-+$/, "");
  return GROUP.test(s) ? s : null;
}

/// A SCIM error, RFC 7644 3.12. `scimType` only where the RFC names one.
export function scimError(status, detail, scimType) {
  const body = { schemas: [ERROR_SCHEMA], status: String(status), detail };
  if (scimType) body.scimType = scimType;
  return { status, body: { scim_error: body } };
}

/// `true`, `false`, or the strings Entra has been known to send for them.
function boolOf(v) {
  if (v === true || v === false) return v;
  if (typeof v === "string" && /^(true|false)$/i.test(v.trim())) return v.trim().toLowerCase() === "true";
  return undefined;
}

// ---------------------------------------------- reading SCIM's requests

/// `userName eq "x"`, `externalId eq "x"`, `displayName eq "x"`, `id eq "x"`
/// (attribute names are case-insensitive; Entra has sent an unquoted
/// value). Anything else is not supported. Answers `{attr, value}`, null
/// for no filter, or `{bad}`.
export function parseFilter(raw) {
  if (raw === null || raw === undefined || String(raw).trim() === "") return null;
  const m = /^\s*(userName|externalId|displayName|id)\s+eq\s+(?:"((?:[^"\\]|\\.)*)"|([^\s"]+))\s*$/i.exec(String(raw));
  if (!m) return { bad: true };
  const attr = { username: "userName", externalid: "externalId", displayname: "displayName", id: "id" }[m[1].toLowerCase()];
  const value = m[2] !== undefined ? m[2].replace(/\\(.)/g, "$1") : m[3];
  return { attr, value };
}

/// The person a User body names: its userName, which must be an email
/// address. Answers the principal, or null.
export function principalOfUserName(userName) {
  const p = parsePrincipal(`email:${String(userName || "").trim()}`);
  return p ? p.principal : null;
}

/// The attributes Krate keeps from a whole User body (POST, PUT). The rest
/// -- passwords Okta sends, titles, phone numbers -- is never stored.
export function userFields(body) {
  const b = body && typeof body === "object" ? body : {};
  const name = b.name && typeof b.name === "object" ? b.name : {};
  const out = {
    userName: b.userName,
    externalId: clip(b.externalId, 200),
    givenName: clip(name.givenName),
    familyName: clip(name.familyName),
    displayName: clip(b.displayName),
  };
  const active = boolOf(b.active);
  out.active = active === undefined ? true : active;
  return out;
}

/// A User PatchOp (RFC 7644 3.5.2) as the changes Krate keeps:
/// `{active?, givenName?, familyName?, displayName?, externalId?, userName?}`.
/// Both Okta's form (`replace` with a value object and no path) and Entra's
/// (`Replace` with a path) are read; op names are case-insensitive. Paths
/// Krate does not keep (emails, title, the enterprise extension) are
/// accepted and ignored. Answers `{changes}` or `{bad}`.
export function userPatch(body) {
  const ops = body && Array.isArray(body.Operations) ? body.Operations : null;
  if (!ops) return { bad: "a PATCH body is a PatchOp with Operations" };
  const changes = {};
  const set = (path, value, removing) => {
    const p = String(path || "").trim();
    const key = p.toLowerCase();
    if (key === "active") {
      if (removing) return null;
      const b = boolOf(value);
      if (b === undefined) return "active is true or false";
      changes.active = b;
    } else if (key === "displayname") changes.displayName = removing ? null : clip(value);
    else if (key === "name.givenname") changes.givenName = removing ? null : clip(value);
    else if (key === "name.familyname") changes.familyName = removing ? null : clip(value);
    else if (key === "externalid") changes.externalId = removing ? null : clip(value, 200);
    else if (key === "username") {
      if (removing) return "userName cannot be removed";
      changes.userName = value;
    } else if (key === "name") {
      const n = value && typeof value === "object" ? value : {};
      if ("givenName" in n || removing) changes.givenName = removing ? null : clip(n.givenName);
      if ("familyName" in n || removing) changes.familyName = removing ? null : clip(n.familyName);
    }
    return null;
  };
  for (const op of ops) {
    const kind = String((op && op.op) || "").toLowerCase();
    if (!["add", "replace", "remove"].includes(kind)) return { bad: `unknown PATCH op ${JSON.stringify(op && op.op)}` };
    const removing = kind === "remove";
    if (op.path === undefined || op.path === null || op.path === "") {
      if (removing) return { bad: "a remove needs a path" };
      if (!op.value || typeof op.value !== "object") return { bad: "a PATCH without a path carries an object value" };
      for (const [k, v] of Object.entries(op.value)) {
        const problem = set(k, v, false);
        if (problem) return { bad: problem };
      }
      continue;
    }
    const problem = set(op.path, op.value, removing);
    if (problem) return { bad: problem };
  }
  return { changes };
}

const idsOf = (list) => (Array.isArray(list) ? list : []).map((m) => (m && typeof m === "object" ? m.value : m)).filter((v) => typeof v === "string" && v);

/// A Group PatchOp as `{displayName?, externalId?, add: [ids], remove: [ids],
/// replace?: [ids]}`, read in both Okta's and Entra's forms: `add` /
/// `remove` on `members` with a value list, Okta's
/// `members[value eq "<id>"]` filter path, and a `replace` with a value
/// object to rename. Answers `{changes}` or `{bad}`.
export function groupPatch(body) {
  const ops = body && Array.isArray(body.Operations) ? body.Operations : null;
  if (!ops) return { bad: "a PATCH body is a PatchOp with Operations" };
  const changes = { add: [], remove: [] };
  for (const op of ops) {
    const kind = String((op && op.op) || "").toLowerCase();
    if (!["add", "replace", "remove"].includes(kind)) return { bad: `unknown PATCH op ${JSON.stringify(op && op.op)}` };
    const path = op.path === undefined || op.path === null ? "" : String(op.path).trim();
    const filtered = /^members\s*\[\s*value\s+eq\s+"((?:[^"\\]|\\.)*)"\s*\]$/i.exec(path);
    if (filtered) {
      if (kind !== "remove") return { bad: "only a remove may name one member by filter" };
      changes.remove.push(filtered[1]);
    } else if (path.toLowerCase() === "members") {
      const ids = idsOf(op.value);
      if (kind === "add") changes.add.push(...ids);
      else if (kind === "remove") {
        // No value: every member goes (RFC 7644 3.5.2.2).
        if (op.value === undefined || op.value === null) changes.replace = [];
        else changes.remove.push(...ids);
      } else changes.replace = ids;
    } else if (path.toLowerCase() === "displayname") {
      if (kind === "remove") return { bad: "a group's displayName cannot be removed" };
      changes.displayName = clip(op.value);
    } else if (path.toLowerCase() === "externalid") {
      changes.externalId = kind === "remove" ? null : clip(op.value, 200);
    } else if (path === "") {
      if (kind === "remove" || !op.value || typeof op.value !== "object") return { bad: "a PATCH without a path carries an object value" };
      if (op.value.displayName !== undefined) changes.displayName = clip(op.value.displayName);
      if (op.value.externalId !== undefined) changes.externalId = clip(op.value.externalId, 200);
      if (op.value.members !== undefined) {
        if (kind === "add") changes.add.push(...idsOf(op.value.members));
        else changes.replace = idsOf(op.value.members);
      }
    }
    // Any other path names something Krate does not keep for a group.
  }
  return { changes };
}

// ------------------------------------------------------------ the store

const ops = {
  scimSettings() {
    return this.row("SELECT * FROM scim_settings WHERE id = 1");
  },

  scimDefaultRole() {
    const s = this.scimSettings();
    return s && SCIM_ROLES.includes(s.default_role) ? s.default_role : SCIM_DEFAULT_ROLE;
  },

  /// The token the request carried, still in force, or a 401. Nothing is
  /// audited for a wrong token: anyone can send one, and the audit must
  /// not grow for them (the door counts them instead).
  scimAuth(ctx) {
    if (!this.org()) return { refusal: scimError(404, `there is no organisation called ${ctx.slug}`) };
    const hash = String(ctx.args.token_hash || "");
    const t = /^[0-9a-f]{64}$/.test(hash) ? this.row("SELECT * FROM scim_tokens WHERE hash = ?", hash) : null;
    if (!t || t.revoked_at) {
      return {
        refusal: scimError(
          401,
          t
            ? `this SCIM token was revoked -- an owner of ${ctx.slug} makes a new one in Settings, Provisioning`
            : `that is not a SCIM token for ${ctx.slug} -- an owner makes one in Settings, Provisioning`,
        ),
      };
    }
    if (!t.last_used_at || ctx.now - t.last_used_at >= 60) {
      this.sql.exec("UPDATE scim_tokens SET last_used_at = ? WHERE id = ?", ctx.now, t.id);
    }
    return { token: t };
  },

  scimUserRow(principal) {
    return this.row("SELECT * FROM scim_users WHERE principal = ?", principal);
  },

  /// Every person the directory can see: members named by an address,
  /// minus those it deleted.
  scimPeople() {
    return this.rows(
      `SELECT m.*, s.external_id, s.given_name, s.family_name, s.display_name, s.created_at AS s_created, s.updated_at AS s_updated
       FROM members m LEFT JOIN scim_users s ON s.principal = m.principal
       WHERE m.principal LIKE 'email:%' AND COALESCE(s.deleted, 0) = 0
       ORDER BY m.added_at, m.rowid`,
    );
  },

  scimPersonById(ctx, id) {
    return this.scimPeople().find((m) => userIdOf(ctx.slug, m.principal) === id) || null;
  },

  scimUser(ctx, m) {
    const email = displayOf(m.principal);
    const id = userIdOf(ctx.slug, m.principal);
    const out = { schemas: [USER_SCHEMA], id };
    if (m.external_id) out.externalId = m.external_id;
    out.userName = email;
    const name = {};
    if (m.given_name) name.givenName = m.given_name;
    if (m.family_name) name.familyName = m.family_name;
    if (Object.keys(name).length) {
      name.formatted = [m.given_name, m.family_name].filter(Boolean).join(" ");
      out.name = name;
    }
    if (m.display_name) out.displayName = m.display_name;
    out.emails = [{ value: email, type: "work", primary: true }];
    out.active = m.state === "active";
    out.meta = {
      resourceType: "User",
      created: iso(m.s_created || m.added_at),
      lastModified: iso(Math.max(m.s_updated || 0, m.added_at || 0, m.removed_at || 0)),
      location: `${ctx.args.base || ""}/Users/${id}`,
    };
    return out;
  },

  scimGroup(ctx, name, { members = true } = {}) {
    const g = this.row("SELECT * FROM groups WHERE name = ?", name);
    const s = this.row("SELECT * FROM scim_groups WHERE name = ?", name);
    const id = groupIdOf(ctx.slug, name);
    const out = { schemas: [GROUP_SCHEMA], id };
    if (s && s.external_id) out.externalId = s.external_id;
    out.displayName = (s && s.display_name) || name;
    if (members) {
      out.members = this.rows("SELECT principal FROM group_members WHERE grp = ? ORDER BY added_at, principal", name)
        .filter((r) => r.principal.startsWith("email:"))
        .map((r) => ({ value: userIdOf(ctx.slug, r.principal), display: displayOf(r.principal) }));
    }
    out.meta = {
      resourceType: "Group",
      created: iso((s && s.created_at) || (g && g.created_at)),
      lastModified: iso((s && s.updated_at) || (g && g.created_at)),
      location: `${ctx.args.base || ""}/Groups/${id}`,
    };
    return out;
  },

  /// The groups the directory manages: those it created or linked.
  scimGroupById(ctx, id) {
    const s = this.rows("SELECT name FROM scim_groups").find((r) => groupIdOf(ctx.slug, r.name) === id);
    return s && this.row("SELECT name FROM groups WHERE name = ?", s.name) ? s.name : null;
  },

  scimKeep(principal, fields, now) {
    const had = this.scimUserRow(principal);
    if (!had) {
      this.sql.exec(
        `INSERT INTO scim_users (principal, external_id, given_name, family_name, display_name, created_at, updated_at, deleted)
         VALUES (?, ?, ?, ?, ?, ?, ?, 0)`,
        principal,
        fields.externalId ?? null,
        fields.givenName ?? null,
        fields.familyName ?? null,
        fields.displayName ?? null,
        now,
        now,
      );
      return;
    }
    const pick = (k, col) => (k in fields ? fields[k] : had[col]);
    this.sql.exec(
      "UPDATE scim_users SET external_id = ?, given_name = ?, family_name = ?, display_name = ?, updated_at = ?, deleted = 0 WHERE principal = ?",
      pick("externalId", "external_id"),
      pick("givenName", "given_name"),
      pick("familyName", "family_name"),
      pick("displayName", "display_name"),
      now,
      principal,
    );
  },

  /// Why the directory may not add a member now, or null: an organisation
  /// that is closing grows no further, and a suspended one adds no paid
  /// seat past its free ones -- the same rules an invitation meets.
  scimGrowRefusal(ctx, role) {
    if (lifecycle(this).state === "closing") {
      return scimError(409, `${ctx.slug} is closing, so nobody new can join -- an owner can cancel the close`);
    }
    const billing = billingState(this, ctx.now);
    if (billing.suspended && SEAT_ROLES.has(role) && seatsUsed(this) + 1 > freeSeats(this.env)) {
      return scimError(409, `${ctx.slug} is suspended for billing, and a seat past its free ones is a paid service -- set the SCIM default role to recipient, or settle billing`);
    }
    return null;
  },

  /// Remove a member as the directory: what an owner's removal does (org.js
  /// op_remove, and org-releases' group clean-up), with `scim` as the actor
  /// and no role matrix -- the directory is the company's word on who works
  /// there. The last owner is never removed.
  scimRemove(ctx, m, why) {
    if (m.role === "owner" && this.activeOwners() <= 1) {
      this.audit("scim", "member.remove", `member:${m.principal}`, "denied", "last-owner");
      return scimError(409, `${displayOf(m.principal)} is ${ctx.slug}'s last owner, and removing them would leave nobody in charge -- make someone else an owner in Krate first`);
    }
    this.sql.exec(
      "UPDATE members SET state = 'removed', removed_at = ?, removed_by = 'scim' WHERE principal = ?",
      ctx.now,
      m.principal,
    );
    this.audit("scim", "member.remove", `member:${m.principal}`, "allowed", `${m.role}; ${why}`);
    for (const invite of this.rows("SELECT code, principal, role FROM invites WHERE issued_by = ? AND state = 'open'", m.principal)) {
      this.sql.exec("UPDATE invites SET state = 'revoked', closed_at = ?, closed_by = 'scim' WHERE code = ?", ctx.now, invite.code);
      this.audit("scim", "invite.revoke", `invite:${invite.principal}/${invite.role}`, "allowed", "issuer removed");
    }
    for (const g of this.rows("SELECT grp FROM group_members WHERE principal = ?", m.principal)) {
      this.sql.exec("DELETE FROM group_members WHERE grp = ? AND principal = ?", g.grp, m.principal);
      this.audit("scim", "group.remove", `group:${g.grp}/${m.principal}`, "allowed", "member removed");
    }
    return null;
  },

  /// Bring back someone the directory removed. Their old role if it is one
  /// the directory may give, the default otherwise.
  scimReactivate(ctx, m) {
    if (m.removed_by !== "scim") {
      return scimError(
        409,
        `${displayOf(m.principal)} was removed from ${ctx.slug} by ${displayOf(m.removed_by) || "an owner"} in Krate, so the directory cannot bring them back -- an owner of ${ctx.slug} invites them again`,
      );
    }
    const role = SCIM_ROLES.includes(m.role) ? m.role : this.scimDefaultRole();
    const refusal = this.scimGrowRefusal(ctx, role);
    if (refusal) return refusal;
    this.sql.exec(
      "UPDATE members SET role = ?, state = 'active', added_at = ?, added_by = 'scim', removed_at = NULL, removed_by = NULL WHERE principal = ?",
      role,
      ctx.now,
      m.principal,
    );
    this.audit("scim", "member.add", `member:${m.principal}`, "allowed", `${role}; reactivated by the directory`);
    return null;
  },

  // ------------------------------------------------- the directory's doors

  "scim-users-list"(ctx) {
    const { refusal } = this.scimAuth(ctx);
    if (refusal) return refusal;
    const f = ctx.args.filter || null;
    let people = this.scimPeople();
    if (f) {
      if (f.attr === "userName") people = people.filter((m) => displayOf(m.principal) === String(f.value).trim().toLowerCase());
      else if (f.attr === "externalId") people = people.filter((m) => m.external_id === f.value);
      else if (f.attr === "id") people = people.filter((m) => userIdOf(ctx.slug, m.principal) === f.value);
      else return scimError(400, "Users can be filtered by userName, externalId or id", "invalidFilter");
    }
    return this.scimList(ctx, people.map((m) => this.scimUser(ctx, m)));
  },

  scimList(ctx, all) {
    const start = Math.max(1, Number.parseInt(ctx.args.startIndex, 10) || 1);
    const countRaw = Number.parseInt(ctx.args.count, 10);
    const count = Number.isFinite(countRaw) ? Math.max(0, Math.min(MAX_COUNT, countRaw)) : 100;
    const page = all.slice(start - 1, start - 1 + count);
    return this.ok({ schemas: [LIST_SCHEMA], totalResults: all.length, startIndex: start, itemsPerPage: page.length, Resources: page });
  },

  "scim-user-get"(ctx) {
    const { refusal } = this.scimAuth(ctx);
    if (refusal) return refusal;
    const m = this.scimPersonById(ctx, String(ctx.args.id || ""));
    if (!m) return scimError(404, `no user with id ${String(ctx.args.id || "").slice(0, 40)} in ${ctx.slug}`);
    return this.ok(this.scimUser(ctx, m));
  },

  /// POST /Users. A new address joins as the SCIM default role; an address
  /// the directory itself removed or deleted comes back; one that is
  /// already here is a 409 (`uniqueness`), as both Okta and Entra expect.
  "scim-user-create"(ctx) {
    const { refusal } = this.scimAuth(ctx);
    if (refusal) return refusal;
    const f = ctx.args.fields || {};
    const principal = principalOfUserName(f.userName);
    if (!principal) {
      return scimError(400, "userName must be the person's email address -- Krate names people by the inbox they sign in with (in Entra map userPrincipalName or mail; in Okta, the email)", "invalidValue");
    }
    const m = this.member(principal);
    const row = this.scimUserRow(principal);
    if (m && !(row && row.deleted)) {
      // The directory sees every member named by an address, invited ones
      // included: it finds them by filter and links, rather than creating.
      return scimError(409, `${displayOf(principal)} is already in ${ctx.slug} -- find them with filter=userName eq "${displayOf(principal)}"`, "uniqueness");
    }
    if (m) {
      // Deleted from the directory's view earlier. Someone it removed comes
      // back; someone an owner removed does not; someone an owner invited
      // back since is adopted with the role the owner gave them.
      if (m.state !== "active" && f.active !== false) {
        const back = this.scimReactivate(ctx, m);
        if (back) return back;
      }
    } else if (f.active !== false) {
      const role = this.scimDefaultRole();
      const grow = this.scimGrowRefusal(ctx, role);
      if (grow) return grow;
      this.sql.exec(
        "INSERT INTO members (principal, role, state, added_at, added_by) VALUES (?, ?, 'active', ?, 'scim')",
        principal,
        role,
        ctx.now,
      );
      this.audit("scim", "member.add", `member:${principal}`, "allowed", `${role}; created by the directory`);
    } else {
      // Created inactive: a record of the person, with no access.
      this.sql.exec(
        "INSERT INTO members (principal, role, state, added_at, added_by, removed_at, removed_by) VALUES (?, ?, 'removed', ?, 'scim', ?, 'scim')",
        principal,
        this.scimDefaultRole(),
        ctx.now,
        ctx.now,
      );
      this.audit("scim", "member.add", `member:${principal}`, "allowed", "created inactive by the directory");
    }
    this.scimKeep(principal, f, ctx.now);
    const after = this.scimPersonById(ctx, userIdOf(ctx.slug, principal));
    return { ...this.ok(this.scimUser(ctx, after), 201), note: this.scimNote(after) };
  },

  /// What the Worker writes to the display index after a change.
  scimNote(m) {
    return m ? { principal: m.principal, role: m.state === "active" ? m.role : null, name: this.org().name } : null;
  },

  /// PATCH and PUT /Users/<id>: `changes` from userPatch or userFields.
  "scim-user-update"(ctx) {
    const { refusal } = this.scimAuth(ctx);
    if (refusal) return refusal;
    const m = this.scimPersonById(ctx, String(ctx.args.id || ""));
    if (!m) return scimError(404, `no user with id ${String(ctx.args.id || "").slice(0, 40)} in ${ctx.slug}`);
    const c = ctx.args.changes || {};
    if (c.userName !== undefined && principalOfUserName(c.userName) !== m.principal) {
      return scimError(
        400,
        "a user's userName is the address they sign in with, and Krate does not change who a member is -- deactivate this user and create one with the new address",
        "mutability",
      );
    }
    if (c.active === false && m.state === "active") {
      const r = this.scimRemove(ctx, m, "deactivated in the directory");
      if (r) return r;
    } else if (c.active === true && m.state !== "active") {
      const r = this.scimReactivate(ctx, m);
      if (r) return r;
    }
    const keep = {};
    for (const k of ["externalId", "givenName", "familyName", "displayName"]) if (k in c) keep[k] = c[k];
    this.scimKeep(m.principal, keep, ctx.now);
    const after = this.scimPersonById(ctx, String(ctx.args.id || ""));
    return { ...this.ok(this.scimUser(ctx, after)), note: this.scimNote(after) };
  },

  /// DELETE /Users/<id>: removed, and gone from the directory's view.
  "scim-user-delete"(ctx) {
    const { refusal } = this.scimAuth(ctx);
    if (refusal) return refusal;
    const m = this.scimPersonById(ctx, String(ctx.args.id || ""));
    if (!m) return scimError(404, `no user with id ${String(ctx.args.id || "").slice(0, 40)} in ${ctx.slug}`);
    if (m.state === "active") {
      const r = this.scimRemove(ctx, m, "deleted in the directory");
      if (r) return r;
    }
    this.scimKeep(m.principal, {}, ctx.now);
    this.sql.exec("UPDATE scim_users SET deleted = 1 WHERE principal = ?", m.principal);
    return { status: 204, body: null, note: { principal: m.principal, role: null } };
  },

  "scim-groups-list"(ctx) {
    const { refusal } = this.scimAuth(ctx);
    if (refusal) return refusal;
    const f = ctx.args.filter || null;
    let names = this.rows("SELECT s.name FROM scim_groups s JOIN groups g ON g.name = s.name ORDER BY s.created_at, s.name").map((r) => r.name);
    if (f) {
      if (f.attr === "displayName") {
        names = names.filter((n) => this.scimGroup(ctx, n, { members: false }).displayName === f.value);
      } else if (f.attr === "externalId") {
        names = names.filter((n) => (this.row("SELECT external_id FROM scim_groups WHERE name = ?", n) || {}).external_id === f.value);
      } else if (f.attr === "id") names = names.filter((n) => groupIdOf(ctx.slug, n) === f.value);
      else return scimError(400, "Groups can be filtered by displayName, externalId or id", "invalidFilter");
    }
    return this.scimList(ctx, names.map((n) => this.scimGroup(ctx, n, { members: !ctx.args.noMembers })));
  },

  "scim-group-get"(ctx) {
    const { refusal } = this.scimAuth(ctx);
    if (refusal) return refusal;
    const name = this.scimGroupById(ctx, String(ctx.args.id || ""));
    if (!name) return scimError(404, `no group with id ${String(ctx.args.id || "").slice(0, 40)} in ${ctx.slug}`);
    return this.ok(this.scimGroup(ctx, name, { members: !ctx.args.noMembers }));
  },

  /// Set a group's members to the directory's (`replace`), or add and
  /// remove some. Only active members join; a deactivated person's place
  /// is skipped, because removal already took them out of every group. An
  /// id that is nobody is a 400, so the directory hears about it.
  scimMembers(ctx, name, { add = [], remove = [], replace }) {
    const people = new Map(this.scimPeople().map((m) => [userIdOf(ctx.slug, m.principal), m]));
    const resolve = (ids) => {
      const out = [];
      for (const id of ids) {
        const m = people.get(id);
        if (!m) return { bad: id };
        out.push(m);
      }
      return { members: out };
    };
    const all = [...add, ...remove, ...(replace || [])];
    if (all.length > MAX_MEMBERS) return scimError(400, `at most ${MAX_MEMBERS} members change in one request`, "tooMany");
    const toAdd = resolve(replace || add);
    if (toAdd.bad) return scimError(400, `no user with id ${String(toAdd.bad).slice(0, 40)} in ${ctx.slug}`, "invalidValue");
    const toRemove = resolve(remove);
    if (toRemove.bad) return scimError(400, `no user with id ${String(toRemove.bad).slice(0, 40)} in ${ctx.slug}`, "invalidValue");
    const current = new Set(this.rows("SELECT principal FROM group_members WHERE grp = ?", name).map((r) => r.principal));
    const drop = replace
      ? [...current].filter((p) => p.startsWith("email:") && !toAdd.members.some((m) => m.principal === p))
      : toRemove.members.map((m) => m.principal).filter((p) => current.has(p));
    for (const p of drop) {
      this.sql.exec("DELETE FROM group_members WHERE grp = ? AND principal = ?", name, p);
      this.audit("scim", "group.remove", `group:${name}/${p}`, "allowed");
      current.delete(p);
    }
    for (const m of toAdd.members) {
      if (m.state !== "active" || current.has(m.principal)) continue;
      this.sql.exec("INSERT INTO group_members (grp, principal, added_at, added_by) VALUES (?, ?, ?, 'scim')", name, m.principal, ctx.now);
      this.audit("scim", "group.add", `group:${name}/${m.principal}`, "allowed");
      current.add(m.principal);
    }
    return null;
  },

  /// POST /Groups. "Finance Team" becomes `finance-team`, or links to the
  /// group of that name an owner already made; a group the directory
  /// already holds is a 409.
  "scim-group-create"(ctx) {
    const { refusal } = this.scimAuth(ctx);
    if (refusal) return refusal;
    const display = clip(ctx.args.displayName);
    const name = groupNameOf(display);
    if (!display || !name) return scimError(400, "a group needs a displayName with at least one letter or digit", "invalidValue");
    if (this.row("SELECT name FROM scim_groups WHERE name = ?", name)) {
      return scimError(409, `${ctx.slug} already has the directory's group ${name}`, "uniqueness");
    }
    if (!this.row("SELECT name FROM groups WHERE name = ?", name)) {
      this.sql.exec("INSERT INTO groups (name, created_at, created_by) VALUES (?, ?, 'scim')", name, ctx.now);
      this.audit("scim", "group.create", `group:${name}`, "allowed");
    } else {
      this.audit("scim", "group.link", `group:${name}`, "allowed", "the directory now manages this group");
    }
    this.sql.exec(
      "INSERT INTO scim_groups (name, display_name, external_id, created_at, updated_at) VALUES (?, ?, ?, ?, ?)",
      name,
      display,
      clip(ctx.args.externalId, 200),
      ctx.now,
      ctx.now,
    );
    const r = this.scimMembers(ctx, name, { replace: ctx.args.members || [] });
    if (r) throw new ScimAbort(r);
    return this.ok(this.scimGroup(ctx, name), 201);
  },

  "scim-group-update"(ctx) {
    const { refusal } = this.scimAuth(ctx);
    if (refusal) return refusal;
    const name = this.scimGroupById(ctx, String(ctx.args.id || ""));
    if (!name) return scimError(404, `no group with id ${String(ctx.args.id || "").slice(0, 40)} in ${ctx.slug}`);
    const c = ctx.args.changes || {};
    const s = this.row("SELECT * FROM scim_groups WHERE name = ?", name);
    if (c.displayName !== undefined || c.externalId !== undefined) {
      this.sql.exec(
        "UPDATE scim_groups SET display_name = ?, external_id = ?, updated_at = ? WHERE name = ?",
        c.displayName ? c.displayName : s.display_name,
        c.externalId !== undefined ? c.externalId : s.external_id,
        ctx.now,
        name,
      );
    }
    const r = this.scimMembers(ctx, name, { add: c.add || [], remove: c.remove || [], replace: c.replace });
    // Nothing of a refused request is kept: the PATCH is one change.
    if (r) throw new ScimAbort(r);
    this.sql.exec("UPDATE scim_groups SET updated_at = ? WHERE name = ?", ctx.now, name);
    return this.ok(this.scimGroup(ctx, name));
  },

  /// DELETE /Groups/<id>: the group goes. A release limited to it then
  /// opens for nobody but reviewers -- it fails closed, never open.
  "scim-group-delete"(ctx) {
    const { refusal } = this.scimAuth(ctx);
    if (refusal) return refusal;
    const name = this.scimGroupById(ctx, String(ctx.args.id || ""));
    if (!name) return scimError(404, `no group with id ${String(ctx.args.id || "").slice(0, 40)} in ${ctx.slug}`);
    this.sql.exec("DELETE FROM group_members WHERE grp = ?", name);
    this.sql.exec("DELETE FROM groups WHERE name = ?", name);
    this.sql.exec("DELETE FROM scim_groups WHERE name = ?", name);
    this.audit("scim", "group.delete", `group:${name}`, "allowed");
    return { status: 204, body: null };
  },

  // ------------------------------------------- the owner's Settings card

  scimOwner(ctx, action, target) {
    const { me, refusal } = this.standing(ctx, action, target);
    if (refusal) return { refusal };
    if (allows(me.role, "scim.manage") !== "yes") {
      return { refusal: this.deny(ctx, action, target, "role-cannot", `only an owner of ${ctx.slug} manages provisioning`) };
    }
    return { me };
  },

  scimView(ctx) {
    const s = this.scimSettings();
    return {
      default_role: this.scimDefaultRole(),
      default_role_set_at: s ? s.set_at : null,
      tokens: this.rows("SELECT * FROM scim_tokens WHERE revoked_at IS NULL ORDER BY created_at DESC").map((t) => ({
        id: t.id,
        hint: t.hint,
        created_at: t.created_at,
        created_by: t.created_by,
        last_used_at: t.last_used_at || null,
      })),
      people: this.scimPeople().filter((m) => this.scimUserRow(m.principal)).length,
      groups: this.row("SELECT COUNT(*) AS n FROM scim_groups").n,
      base_url: ctx.args.scim_base || null,
    };
  },

  /// GET /orgs/<slug>/scim (owner).
  "scim-settings"(ctx) {
    const { refusal } = this.scimOwner(ctx, "scim.view", "scim");
    if (refusal) return refusal;
    return this.ok(this.scimView(ctx));
  },

  /// POST /orgs/<slug>/scim/tokens (owner): the Worker made the token and
  /// sends only its hash; the answer the Worker adds the token to is never
  /// stored, so this command takes no Idempotency-Key.
  "scim-token-create"(ctx) {
    const { me, refusal } = this.scimOwner(ctx, "scim.token.create", "scim");
    if (refusal) return refusal;
    const hash = String(ctx.args.hash || "");
    const id = String(ctx.args.id || "");
    if (!/^[0-9a-f]{64}$/.test(hash) || !/^st_[a-z0-9]{8,32}$/.test(id)) return { status: 400, body: { error: "bad-request", message: "not a token" } };
    if (this.rows("SELECT id FROM scim_tokens WHERE revoked_at IS NULL").length >= 5) {
      return this.deny(ctx, "scim.token.create", "scim", "role-cannot", `${ctx.slug} already has 5 SCIM tokens -- revoke one you no longer use first`);
    }
    this.sql.exec(
      "INSERT INTO scim_tokens (id, hash, hint, created_at, created_by) VALUES (?, ?, ?, ?, ?)",
      id,
      hash,
      String(ctx.args.hint || "").slice(0, 16),
      ctx.now,
      me.principal,
    );
    this.audit(me.principal, "scim.token.create", `scim-token:${id}`, "allowed");
    return this.ok({ id, created_at: ctx.now, settings: this.scimView(ctx) }, 201);
  },

  /// DELETE /orgs/<slug>/scim/tokens/<id> (owner): stops at its next request.
  "scim-token-revoke"(ctx) {
    const id = String(ctx.args.id || "");
    const target = `scim-token:${id.slice(0, 40)}`;
    const { me, refusal } = this.scimOwner(ctx, "scim.token.revoke", target);
    if (refusal) return refusal;
    const t = this.row("SELECT * FROM scim_tokens WHERE id = ?", id);
    if (!t) return { status: 404, body: { error: "no-such-token", message: `${ctx.slug} has no SCIM token with that id` } };
    if (!t.revoked_at) {
      this.sql.exec("UPDATE scim_tokens SET revoked_at = ?, revoked_by = ? WHERE id = ?", ctx.now, me.principal, id);
      this.audit(me.principal, "scim.token.revoke", target, "allowed");
    }
    return this.ok(null, 204);
  },

  /// PUT /orgs/<slug>/scim {default_role} (owner).
  "scim-default-set"(ctx) {
    const role = String(ctx.args.default_role || "");
    const { me, refusal } = this.scimOwner(ctx, "scim.default-role", "scim");
    if (refusal) return refusal;
    if (!SCIM_ROLES.includes(role)) {
      return this.deny(
        ctx,
        "scim.default-role",
        "scim",
        "role-cannot",
        role === "owner" || role === "admin"
          ? `the directory can never make someone ${role === "owner" ? "an owner" : "an admin"} -- an owner of ${ctx.slug} does that in People`
          : `the SCIM default role is developer, reviewer or recipient`,
      );
    }
    if (this.scimDefaultRole() !== role || !this.scimSettings()) {
      this.sql.exec(
        `INSERT INTO scim_settings (id, default_role, set_at, set_by) VALUES (1, ?, ?, ?)
         ON CONFLICT (id) DO UPDATE SET default_role = excluded.default_role, set_at = excluded.set_at, set_by = excluded.set_by`,
        role,
        ctx.now,
        me.principal,
      );
      this.audit(me.principal, "scim.default-role", "scim", "allowed", role);
    }
    return this.ok(this.scimView(ctx));
  },
};

/// A refusal found part way through a command that already wrote: thrown,
/// so the store's transaction rolls the whole command back, and answered.
class ScimAbort extends Error {
  constructor(answer) {
    super("scim refusal");
    this.answer = answer;
  }
}

function install(OrgClass) {
  const helpers = {};
  const commands = {};
  for (const [k, fn] of Object.entries(ops)) (k.startsWith("scim-") ? commands : helpers)[k] = fn;
  Object.assign(OrgClass.prototype, helpers);
  // A ScimAbort rolls back what the command wrote (the store's
  // transaction is the one the command runs in) and answers its refusal.
  // The DO's fetch is what calls transactionSync, so the throw is caught
  // there, after the rollback.
  extendOrg({ schema: SCHEMA, ops: commands, mutations: ["scim-token-revoke", "scim-default-set"] });
  const fetch = OrgClass.prototype.fetch;
  OrgClass.prototype.fetch = async function (request) {
    try {
      return await fetch.call(this, request);
    } catch (err) {
      if (err instanceof ScimAbort) {
        return new Response(JSON.stringify(err.answer), { headers: { "content-type": "application/json" } });
      }
      throw err;
    }
  };
}

install(Org);

// ---------------------------------------------------------------- routes

const scimJson = (body, status = 200, extra = {}) =>
  new Response(body === null ? null : JSON.stringify(body), {
    status,
    headers: { "content-type": "application/scim+json", "cache-control": "no-store", ...extra },
  });

/// An answer from the store in SCIM's shape. A refusal in the hub's own
/// shape -- the lifecycle gate's `org-closed`, say -- is carried across in
/// words.
function scimAnswer(answer) {
  if (answer.body && answer.body.scim_error) return scimJson(answer.body.scim_error, answer.status);
  if (answer.status === 204 || answer.body === null) return new Response(null, { status: answer.status === 200 ? 204 : answer.status });
  if (answer.status >= 400) {
    return scimJson({ schemas: [ERROR_SCHEMA], status: String(answer.status), detail: answer.body.message || answer.body.error || "refused" }, answer.status);
  }
  return scimJson(answer.body, answer.status);
}

async function sha256Hex(text) {
  const d = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text));
  return [...new Uint8Array(d)].map((b) => b.toString(16).padStart(2, "0")).join("");
}

const b64url = (bytes) => btoa(String.fromCharCode(...bytes)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");

/// A new SCIM token: `kscim_<43 characters>`, 256 random bits.
export function newScimToken() {
  return `${TOKEN_PREFIX}${b64url(crypto.getRandomValues(new Uint8Array(32)))}`;
}

const TOKEN = /^kscim_[A-Za-z0-9_-]{43}$/;

const SERVICE_PROVIDER_CONFIG = (docs) => ({
  schemas: ["urn:ietf:params:scim:schemas:core:2.0:ServiceProviderConfig"],
  documentationUri: docs,
  patch: { supported: true },
  bulk: { supported: false, maxOperations: 0, maxPayloadSize: 0 },
  filter: { supported: true, maxResults: MAX_COUNT },
  changePassword: { supported: false },
  sort: { supported: false },
  etag: { supported: false },
  authenticationSchemes: [
    { type: "oauthbearertoken", name: "Bearer token", description: "A SCIM token an owner makes in the organisation's Settings, Provisioning", primary: true },
  ],
  meta: { resourceType: "ServiceProviderConfig" },
});

/// /scim/v2/<slug>/... Answers a Response. `deps` are the hub's helpers:
/// orgAsk, readBounded, noteOrgOf, rateLimited, slugOk.
export async function scimRoute(request, url, env, deps) {
  const parts = url.pathname.split("/").filter(Boolean);
  // ["scim", "v2", slug, Resource, id?]
  const slug = String(parts[2] || "").toLowerCase();
  if (!deps.slugOk(slug)) return scimJson({ schemas: [ERROR_SCHEMA], status: "404", detail: "no such organisation" }, 404);
  const resource = parts[3] || "";
  const id = parts[4] ? decodeURIComponent(parts[4]) : null;
  const method = request.method;
  const base = `${String(env.PUBLIC_BASE || "").replace(/\/$/, "")}/scim/v2/${slug}`;

  const auth = request.headers.get("authorization") || "";
  const token = auth.replace(/^Bearer\s+/i, "").trim();
  const refused = async (detail) => {
    const over = await deps.rateLimited(request, env, "scim/refused");
    if (over) return scimJson({ schemas: [ERROR_SCHEMA], status: "429", detail: "too many refused requests -- wait a minute" }, 429, { "retry-after": "60" });
    return scimJson({ schemas: [ERROR_SCHEMA], status: "401", detail }, 401, { "www-authenticate": 'Bearer realm="krate-scim"' });
  };
  if (!TOKEN.test(token)) return refused(`send the SCIM token an owner of ${slug} made in Settings, Provisioning, as Authorization: Bearer kscim_...`);
  const token_hash = await sha256Hex(token);

  const ask = async (op, args = {}) => {
    const answer = await deps.orgAsk(env, slug, { op, principals: [], args: { ...args, token_hash, base } });
    if (answer.status === 401) {
      const r = await refused(answer.body && answer.body.scim_error ? answer.body.scim_error.detail : "not a SCIM token");
      return { response: r };
    }
    if (answer.note && deps.noteOrgOf) await deps.noteOrgOf(env, answer.note.principal, slug, answer.note.name, answer.note.role);
    return { response: scimAnswer(answer) };
  };
  const body = async () => {
    const read = await deps.readBounded(request, 256 * 1024);
    if (read.tooLarge) return { bad: scimJson({ schemas: [ERROR_SCHEMA], status: "413", detail: "the body is over 256 KiB" }, 413) };
    try {
      const v = JSON.parse(new TextDecoder().decode(read.body) || "{}");
      if (!v || typeof v !== "object") throw new Error("not an object");
      return { value: v };
    } catch (_) {
      return { bad: scimJson({ schemas: [ERROR_SCHEMA], status: "400", scimType: "invalidSyntax", detail: "the body is not JSON" }, 400) };
    }
  };
  const listArgs = () => {
    const filter = parseFilter(url.searchParams.get("filter"));
    return {
      filter,
      startIndex: url.searchParams.get("startIndex"),
      count: url.searchParams.get("count"),
      noMembers: /(^|,)\s*members\s*(,|$)/i.test(url.searchParams.get("excludedAttributes") || ""),
    };
  };
  const badFilter = () => scimJson({ schemas: [ERROR_SCHEMA], status: "400", scimType: "invalidFilter", detail: 'only "<attribute> eq \\"value\\"" filters are supported (userName, externalId, displayName, id)' }, 400);

  if (resource === "ServiceProviderConfig" && method === "GET" && !id) {
    // Answered only for a good token: a stranger learns nothing here.
    const check = await ask("scim-users-list", { count: 0 });
    if (check.response.status !== 200) return check.response;
    return scimJson(SERVICE_PROVIDER_CONFIG(`${String(env.SITE_BASE || "https://krate.tech").replace(/\/$/, "")}/docs/organisations.html#provisioning`));
  }
  if (resource === "Users") {
    if (!id && method === "GET") {
      const a = listArgs();
      if (a.filter && a.filter.bad) return badFilter();
      return (await ask("scim-users-list", a)).response;
    }
    if (!id && method === "POST") {
      const b = await body();
      if (b.bad) return b.bad;
      return (await ask("scim-user-create", { fields: userFields(b.value) })).response;
    }
    if (id && method === "GET") return (await ask("scim-user-get", { id })).response;
    if (id && method === "PUT") {
      const b = await body();
      if (b.bad) return b.bad;
      const f = userFields(b.value);
      const changes = { userName: f.userName, externalId: f.externalId, givenName: f.givenName, familyName: f.familyName, displayName: f.displayName, active: f.active };
      return (await ask("scim-user-update", { id, changes })).response;
    }
    if (id && method === "PATCH") {
      const b = await body();
      if (b.bad) return b.bad;
      const p = userPatch(b.value);
      if (p.bad) return scimJson({ schemas: [ERROR_SCHEMA], status: "400", scimType: "invalidSyntax", detail: p.bad }, 400);
      return (await ask("scim-user-update", { id, changes: p.changes })).response;
    }
    if (id && method === "DELETE") return (await ask("scim-user-delete", { id })).response;
  }
  if (resource === "Groups") {
    if (!id && method === "GET") {
      const a = listArgs();
      if (a.filter && a.filter.bad) return badFilter();
      return (await ask("scim-groups-list", a)).response;
    }
    if (!id && method === "POST") {
      const b = await body();
      if (b.bad) return b.bad;
      return (await ask("scim-group-create", { displayName: b.value.displayName, externalId: b.value.externalId, members: idsOf(b.value.members) })).response;
    }
    if (id && method === "GET") {
      const a = listArgs();
      return (await ask("scim-group-get", { id, noMembers: a.noMembers })).response;
    }
    if (id && method === "PUT") {
      const b = await body();
      if (b.bad) return b.bad;
      const changes = { displayName: clip(b.value.displayName), externalId: b.value.externalId === undefined ? undefined : clip(b.value.externalId, 200), replace: idsOf(b.value.members) };
      return (await ask("scim-group-update", { id, changes })).response;
    }
    if (id && method === "PATCH") {
      const b = await body();
      if (b.bad) return b.bad;
      const p = groupPatch(b.value);
      if (p.bad) return scimJson({ schemas: [ERROR_SCHEMA], status: "400", scimType: "invalidSyntax", detail: p.bad }, 400);
      const answer = (await ask("scim-group-update", { id, changes: p.changes })).response;
      // Entra and Okta both take 204 or 200 for a group PATCH; 204 keeps a
      // large group's member list off the wire.
      return answer.status === 200 ? new Response(null, { status: 204 }) : answer;
    }
    if (id && method === "DELETE") return (await ask("scim-group-delete", { id })).response;
  }
  return scimJson({ schemas: [ERROR_SCHEMA], status: "404", detail: `no SCIM endpoint ${method} ${url.pathname}` }, 404);
}

/// /orgs/<slug>/scim, /scim/tokens, /scim/tokens/<id>: the owner's card.
export async function orgScimRoute(h) {
  const { rest, method } = h;
  if (rest[0] !== "scim") return null;
  const scim_base = `${String(h.env.PUBLIC_BASE || "").replace(/\/$/, "")}/scim/v2/${h.slug}`;
  if (rest.length === 1 && method === "GET") return h.orgAnswer(await h.askRaw("scim-settings", { scim_base }));
  if (rest.length === 1 && method === "PUT") return h.orgAnswer(await h.mutateRaw("scim-default-set", (b) => ({ default_role: b.default_role, scim_base })));
  if (rest.length === 2 && rest[1] === "tokens" && method === "POST") {
    const token = newScimToken();
    const id = `st_${[...crypto.getRandomValues(new Uint8Array(10))].map((b) => "abcdefghjkmnpqrstuvwxyz23456789"[b % 31]).join("")}`;
    const answer = await h.askRaw("scim-token-create", { hash: await sha256Hex(token), id, hint: `${token.slice(0, 10)}...`, scim_base });
    if (answer.status !== 201) return h.orgAnswer(answer);
    // The token itself is in this answer and nowhere else, ever.
    const res = h.json({ ...answer.body, token, base_url: scim_base }, 201);
    res.headers.set("cache-control", "no-store");
    return res;
  }
  if (rest.length === 3 && rest[1] === "tokens" && method === "DELETE") {
    return h.orgAnswer(await h.mutateRaw("scim-token-revoke", () => ({ id: rest[2] })));
  }
  return null;
}

export { PATCH_SCHEMA };
