// The doors for an organisation's lifecycle (IC-948, Addendum 3 D, shapes
// from Addendum 4): close with approval and a window, the standard export,
// recovery of a lost owner by Krate support, and the scheduled sweep that
// deletes a closed organisation's files once its export window has passed.
// The store (org-lifecycle-store.js) decides; these routes read and write
// what the store cannot: R2 bytes, the zip, and mail.
//
// `h` is what index.js's organisation router hands every module: the
// request, env, slug, the caller's principals, and its helpers.

import { mailRecovery } from "./org-mail.js";
import { chainOk } from "./org.js";

/// The KV index of organisations with a close in progress, so the nightly
/// sweep can find them. Display and scheduling only: the store decides.
const closingKey = (slug) => `org-closing:${slug}`;

const exportKey = (slug, id) => `org-exports/${slug}/${id}.zip`;

export async function orgLifecycleRoute(h) {
  const { method, rest } = h;
  if (rest[0] === "close") {
    if (rest.length === 1 && method === "POST") {
      const answer = await h.mutateRaw("close", (b) => ({ confirm_slug: b.confirm_slug }));
      if (answer.status < 300 && answer.body.close && answer.body.close.state === "scheduled") await h.env.APPS.put(closingKey(h.slug), "1").catch(() => {});
      return h.orgAnswer(answer);
    }
    if (rest.length === 3 && rest[2] === "approve" && method === "POST") {
      const answer = await h.mutateRaw("close-approve", (b) => ({ id: rest[1], statement_digest: b.statement_digest }));
      if (answer.status < 300) await h.env.APPS.put(closingKey(h.slug), "1").catch(() => {});
      return h.orgAnswer(answer);
    }
    if (rest.length === 2 && method === "DELETE") {
      const answer = await h.mutateRaw("close-cancel", () => ({ id: rest[1] }));
      if (answer.status === 204) await h.env.APPS.delete(closingKey(h.slug)).catch(() => {});
      return h.orgAnswer(answer);
    }
    return null;
  }
  if (rest[0] === "export" && rest.length === 1 && method === "POST") return buildExport(h);
  if (rest[0] === "exports") {
    if (rest.length === 1 && method === "GET") return h.orgAnswer(await h.askRaw("exports"));
    if (rest.length === 2 && method === "GET") {
      const answer = await h.askRaw("export-get", { id: rest[1] });
      if (answer.status !== 200) return h.orgAnswer(answer);
      const object = await h.env.BUNDLES.get(exportKey(h.slug, rest[1]));
      if (!object) return h.json({ error: "export-not-ready", message: "that package is gone -- build a new export" }, 409);
      return new Response(object.body, {
        headers: {
          "content-type": "application/zip",
          "content-disposition": `attachment; filename="${h.slug}-export-${rest[1]}.zip"`,
          "content-length": String(object.size),
          "cache-control": "private, no-store",
        },
      });
    }
    return null;
  }
  if (rest[0] === "recover") {
    if (rest.length === 1 && method === "POST") return startRecovery(h);
    if (rest.length === 3 && rest[2] === "object" && method === "POST") {
      return h.orgAnswer(await h.mutateRaw("recover-object", () => ({ id: rest[1] })));
    }
    return null;
  }
  return null;
}

// ------------------------------------------------------------- recovery

/// POST /orgs/<slug>/recover {principal, reason}: a Krate admin only --
/// GitHub-vouched and in KRATE_ADMINS. Nobody gains anything now; the
/// store makes the principal an owner after 72 hours unless an owner or
/// admin objects, and everyone it can reach is mailed.
async function startRecovery(h) {
  const admin = await h.isAdmin(h.request, h.env);
  if (!admin) {
    return h.json({ error: "role-cannot", message: "only Krate support can recover an organisation's ownership" }, 403);
  }
  const body = await h.readBody();
  if (body.bad || body.tooLarge) return h.json({ error: "bad-request", message: "the body is JSON, under 64 KiB" }, 400);
  const answer = await h.orgAsk(h.env, h.slug, {
    op: "recover-start",
    args: { principal: body.value.principal, reason: body.value.reason, admin: `github:${String(admin.login).toLowerCase()}` },
  });
  if (answer.status !== 201) return h.orgAnswer(answer);
  const mail = await mailRecovery(h.env, h.smtpSend, {
    slug: h.slug,
    orgName: answer.body.org_name,
    recovery: answer.body.recovery,
    notify: answer.body.notify,
    site: h.site,
  });
  const noted = await h.orgAsk(h.env, h.slug, {
    op: "recover-noted",
    args: { id: answer.body.recovery.id, not_mailed: mail.not_mailed },
  });
  const recovery = (noted.body && noted.body.recovery) || answer.body.recovery;
  return h.json({ recovery, mailed: mail.mailed, not_mailed: mail.not_mailed }, 201);
}

// --------------------------------------------------------------- export

/// The most release bytes one package carries (ORG_EXPORT_MAX_BYTES, 2
/// GiB). Memory is not what bounds it -- the zip is streamed into R2 a part
/// at a time -- but a package somebody has to download is. A release past
/// it is listed in the manifest's exclusions with its digest and size,
/// never dropped silently.
const maxExportBytes = (env) => {
  const n = Number(env.ORG_EXPORT_MAX_BYTES);
  return Number.isFinite(n) && n > 0 ? n : 2 * 1024 * 1024 * 1024;
};

export const EXPORT_FORMAT = "krate.org-export.v1";

/// What the standard export never carries, said in the manifest (R28).
export const EXCLUSIONS = [
  { class: "signing keys", why: "private keys never reach Krate; the organisation's root public key is not part of the export either" },
  { class: "tokens and sessions", why: "sign-in tokens and sessions are credentials, never exported" },
  { class: "secrets", why: "no secret held for the organisation (sign-in client secrets included) is exported" },
  { class: "invitation codes", why: "an open invitation's code is a credential; who was invited is in the audit" },
  { class: "idempotency records", why: "request bookkeeping, not organisation data" },
  { class: "billing identifiers", why: "Stripe customer and subscription ids stay with Krate; invoices come from Stripe" },
];

async function buildExport(h) {
  const begun = await h.askRaw("export-begin");
  if (begun.status !== 202) return h.orgAnswer(begun);
  const { id } = begun.body.export;
  const data = begun.body.data;
  let done;
  try {
    const { size } = await streamExport(h.env, h.slug, id, data);
    done = await h.orgAsk(h.env, h.slug, { op: "export-done", args: { id, state: "ready", size } });
  } catch (err) {
    done = await h.orgAsk(h.env, h.slug, {
      op: "export-done",
      args: { id, state: "failed", problem: String((err && err.message) || err).slice(0, 200) },
    });
  }
  return h.json({ export: done.body.export }, 202);
}

/// The largest part the zip is written to R2 in, and so the most of it held
/// in memory at once (plus one chunk read from a release). R2 wants every
/// part but the last the same size and at least 5 MiB.
export const EXPORT_PART_BYTES = 8 * 1024 * 1024;

/// What the last export held in memory at its peak, for the tests that
/// prove the writer is bounded however large the releases are.
export const exportStats = { peakBuffered: 0, parts: 0 };

/// Bytes written into R2 one fixed-size part at a time (multipart, as the
/// big-file path does), so a package of any size never sits whole in a
/// Worker's 128 MB.
class PartWriter {
  constructor(upload, partSize) {
    this.upload = upload;
    this.partSize = partSize;
    this.buffer = new Uint8Array(partSize);
    this.fill = 0;
    this.parts = [];
    this.total = 0;
    this.peak = 0;
  }

  async write(bytes) {
    this.peak = Math.max(this.peak, this.fill + bytes.length);
    let at = 0;
    while (at < bytes.length) {
      const take = Math.min(this.partSize - this.fill, bytes.length - at);
      this.buffer.set(bytes.subarray(at, at + take), this.fill);
      this.fill += take;
      at += take;
      this.total += take;
      if (this.fill === this.partSize) await this.flush();
    }
  }

  async flush() {
    if (!this.fill) return;
    const n = this.parts.length + 1;
    const part = await this.upload.uploadPart(n, this.buffer.slice(0, this.fill));
    this.parts.push({ partNumber: n, etag: part.etag });
    this.fill = 0;
  }

  async finish() {
    await this.flush();
    return this.upload.complete(this.parts);
  }
}

/// Every chunk of an R2 object's body, without holding more than one.
async function* chunksOf(object) {
  const body = object.body;
  if (body && typeof body.getReader === "function") {
    const reader = body.getReader();
    for (;;) {
      const { done, value } = await reader.read();
      if (done) return;
      yield value instanceof Uint8Array ? value : new Uint8Array(value);
    }
  } else if (body) {
    yield body instanceof Uint8Array ? body : new Uint8Array(await new Response(body).arrayBuffer());
  }
}

/// The standard export, written straight into R2 at org-exports/<slug>/<id>.zip:
/// export-manifest.json first, then each data class as JSON, then the
/// releases' own .krate files streamed from the store. A release's CRC is
/// taken in a first streaming pass so its header can be written before its
/// bytes; nothing is ever held whole. Answers `{ size }`.
export async function streamExport(env, slug, id, data) {
  const enc = new TextEncoder();
  const asJson = (v) => enc.encode(JSON.stringify(v, null, 2) + "\n");
  const small = [];
  const classes = [];
  const describe = (cls, path, size, sha256) => {
    let entry = classes.find((c) => c.class === cls);
    if (!entry) classes.push((entry = { class: cls, files: [] }));
    entry.files.push({ path, size, sha256 });
  };
  const addSmall = async (cls, path, bytes) => {
    small.push({ path, bytes });
    describe(cls, path, bytes.length, await sha256Hex(bytes));
  };
  await addSmall("organisation", "org.json", asJson(data.org));
  await addSmall("members and roles", "members.json", asJson(data.members));
  await addSmall("candidates", "candidates.json", asJson(data.candidates));
  const chain = await chainOk(data.audit);
  await addSmall("audit", "audit.json", asJson({ chain_ok: chain, events: data.audit }));
  // The apps' team data (store.org, IC-951): every row, who wrote it, when.
  if (Array.isArray(data.app_data)) await addSmall("app data", "app-data.json", asJson(data.app_data));
  // The keyring (F-316): public keys and sealed envelopes; opening any of it
  // needs a member device or the recovery code.
  if (data.keyring) await addSmall("keyring", "keyring.json", asJson(data.keyring));

  // The releases: their digests are their sha256 (content-addressed), and
  // their sizes are the store's, so the manifest can be written first.
  const excluded = [...EXCLUSIONS];
  const releases = [];
  let budget = maxExportBytes(env);
  for (const c of data.candidates.filter((x) => x.state === "released")) {
    const head = await releaseHead(env, c.digest);
    const name = `releases/${String(c.name || "app").toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "") || "app"}-${String(c.version || "").replace(/[^0-9A-Za-z.]+/g, "-")}-${c.digest.slice(0, 12)}.krate`;
    if (!head) {
      excluded.push({ class: "release file", digest: c.digest, why: "the bytes were not found in the store" });
      continue;
    }
    if (head.size > budget) {
      excluded.push({ class: "release file", digest: c.digest, size: head.size, why: "past the package's size limit; download it as a release while the organisation is open" });
      continue;
    }
    budget -= head.size;
    releases.push({ path: name, digest: c.digest, size: head.size });
    describe("releases", name, head.size, c.digest);
  }
  const manifest = {
    format: EXPORT_FORMAT,
    export_id: id,
    org: data.org.slug,
    generated_at: Math.floor(Date.now() / 1000),
    classes,
    audit_chain_ok: chain,
    excluded,
  };

  const upload = await env.BUNDLES.createMultipartUpload(exportKey(slug, id), {
    httpMetadata: { contentType: "application/zip" },
    customMetadata: { org: slug, export: id },
  });
  const out = new PartWriter(upload, EXPORT_PART_BYTES);
  const central = [];
  const entry = async (path, size, crc, write) => {
    const name = enc.encode(path);
    central.push({ name, size, crc, offset: out.total });
    await out.write(localHeader(name, size, crc));
    await write();
  };
  try {
    for (const f of [{ path: "export-manifest.json", bytes: asJson(manifest) }, ...small]) {
      await entry(f.path, f.bytes.length, crc32(f.bytes), () => out.write(f.bytes));
    }
    for (const r of releases) {
      // Pass one: the CRC. Pass two: the bytes. Neither holds more than a chunk.
      let crc = 0xffffffff;
      for await (const chunk of chunksOf(await releaseObject(env, r.digest))) crc = crcUpdate(crc, chunk);
      await entry(r.path, r.size, (crc ^ 0xffffffff) >>> 0, async () => {
        let written = 0;
        for await (const chunk of chunksOf(await releaseObject(env, r.digest))) {
          await out.write(chunk);
          written += chunk.length;
        }
        if (written !== r.size) throw new Error(`release ${r.digest.slice(0, 12)} changed size while it was exported`);
      });
    }
    const directoryAt = out.total;
    for (const c of central) await out.write(centralRecord(c));
    await out.write(endRecord(central.length, out.total - directoryAt, directoryAt));
    await out.finish();
  } catch (err) {
    try {
      await upload.abort();
    } catch (_) {}
    throw err;
  } finally {
    exportStats.peakBuffered = out.peak;
    exportStats.parts = out.parts.length;
  }
  return { size: out.total };
}

async function releaseHead(env, digest) {
  let object = await env.BUNDLES.head(digest);
  if (!object) {
    const blob = await env.APPS.get(`bigkey:${digest}`).catch(() => null);
    if (blob) object = await env.BUNDLES.head(blob);
  }
  return object;
}

async function releaseObject(env, digest) {
  let object = await env.BUNDLES.get(digest);
  if (!object) {
    const blob = await env.APPS.get(`bigkey:${digest}`).catch(() => null);
    if (blob) object = await env.BUNDLES.get(blob);
  }
  return object;
}

// ------------------------------------------------------------- the sweep

/// The nightly sweep: for every organisation with a close in progress, let
/// its store apply what is due, and once a closed organisation's export
/// window has passed, delete its release bytes and export packages and let
/// the store record what was deleted.
export async function sweepOrgs(env, orgAsk) {
  if (!env.ORGS) return [];
  let listed;
  try {
    listed = await env.APPS.list({ prefix: "org-closing:" });
  } catch (_) {
    return [];
  }
  const out = [];
  for (const { name } of listed.keys || []) {
    const slug = name.slice("org-closing:".length);
    const due = await orgAsk(env, slug, { op: "sweep" });
    if (!due.body || !due.body.due) continue;
    const deleted = [];
    for (const { digest } of due.body.digests) {
      const head = await env.BUNDLES.head(digest);
      if (head && head.customMetadata && head.customMetadata.org === slug) {
        await env.BUNDLES.delete(digest);
        deleted.push(digest);
      }
      // Its code comparison (org-approvals.js) holds source: it goes too.
      const changes = await env.BUNDLES.head(`changes:${digest}`);
      if (changes && changes.customMetadata && changes.customMetadata.org === slug) await env.BUNDLES.delete(`changes:${digest}`);
      const blob = await env.APPS.get(`bigkey:${digest}`).catch(() => null);
      if (blob) {
        const bigHead = await env.BUNDLES.head(blob);
        if (bigHead && bigHead.customMetadata && bigHead.customMetadata.org === slug) {
          await env.BUNDLES.delete(blob);
          await env.APPS.delete(`bigkey:${digest}`);
          deleted.push(digest);
        }
      }
    }
    for (const id of due.body.exports) {
      await env.BUNDLES.delete(exportKey(slug, id));
      deleted.push(`export:${id}`);
    }
    await orgAsk(env, slug, { op: "swept", args: { deleted } });
    await env.APPS.delete(name);
    out.push({ slug, deleted });
  }
  return out;
}

// ------------------------------------------------------------ zip, sha

async function sha256Hex(bytes) {
  const digest = await crypto.subtle.digest("SHA-256", bytes);
  return [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, "0")).join("");
}

const CRC = (() => {
  const t = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c >>> 0;
  }
  return t;
})();

function crcUpdate(c, bytes) {
  for (let i = 0; i < bytes.length; i++) c = CRC[(c ^ bytes[i]) & 0xff] ^ (c >>> 8);
  return c;
}

function crc32(bytes) {
  return (crcUpdate(0xffffffff, bytes) ^ 0xffffffff) >>> 0;
}

// A zip of stored (uncompressed) entries: the .krate files inside are
// already compressed. Sizes and CRC are known before each entry is
// written, so there are no data descriptors.

function localHeader(name, size, crc) {
  const v = new DataView(new ArrayBuffer(30));
  v.setUint32(0, 0x04034b50, true);
  v.setUint16(4, 20, true);
  v.setUint16(6, 0x0800, true); // names are UTF-8
  v.setUint16(8, 0, true); // stored
  v.setUint32(14, crc, true);
  v.setUint32(18, size, true);
  v.setUint32(22, size, true);
  v.setUint16(26, name.length, true);
  return concat(new Uint8Array(v.buffer), name);
}

function centralRecord({ name, size, crc, offset }) {
  const v = new DataView(new ArrayBuffer(46));
  v.setUint32(0, 0x02014b50, true);
  v.setUint16(4, 20, true);
  v.setUint16(6, 20, true);
  v.setUint16(8, 0x0800, true);
  v.setUint32(16, crc, true);
  v.setUint32(20, size, true);
  v.setUint32(24, size, true);
  v.setUint16(28, name.length, true);
  v.setUint32(42, offset, true);
  return concat(new Uint8Array(v.buffer), name);
}

function endRecord(count, size, offset) {
  const v = new DataView(new ArrayBuffer(22));
  v.setUint32(0, 0x06054b50, true);
  v.setUint16(8, count, true);
  v.setUint16(10, count, true);
  v.setUint32(12, size, true);
  v.setUint32(16, offset, true);
  return new Uint8Array(v.buffer);
}

function concat(a, b) {
  const out = new Uint8Array(a.length + b.length);
  out.set(a, 0);
  out.set(b, a.length);
  return out;
}
