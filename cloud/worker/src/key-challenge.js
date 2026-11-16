// Proof that the person importing an Android app holds its signing key
// (IC-800, portable test 1717).
//
// POST /port/key-challenge issues a statement -- package, certificate, the
// exact APK's digest, the import request, the account asking, a one-time
// nonce and an expiry -- and keeps it under its nonce for fifteen minutes.
// The person signs a small APK carrying it with their own tools and key
// (apksigner, Gradle, Android Studio); POST /port/key-challenge/answer takes
// that signed APK, and the validator (crates/apksig, the same verifier
// `krate port` runs) checks it carries exactly the statement issued and is
// signed by that certificate or a key it rotated to. The record is deleted
// on the first answer, right or wrong, so a nonce is never used twice.
//
// What it answers is the hub's signed statement of key control, under the
// hub's Ed25519 key (ORG_LEASE_KEY, published at /lease-key) with its own
// domain, "krate-key-control-v1\n", so it can never be read as a lease. It
// says who held the key, for which package and which APK, and when -- and
// nothing about who owns the app in law, a separate question (IC-801).

import { leaseKey } from "./org-lease.js";
import { canonicalJson } from "./org.js";

export const KEY_CONTROL_DOMAIN = "krate-key-control-v1\n";
export const CHALLENGE_SECONDS = 15 * 60;
export const MAX_ANSWER_BYTES = 256 * 1024;

/// The words every statement carries; the same as crates/apksig's PURPOSE.
export const PURPOSE =
  "Signing this proves to Krate (krate.tech) that you hold this app's signing key, for one import. It cannot be installed and grants nothing else.";

const HEX64 = /^[0-9a-f]{64}$/;
const PACKAGE = /^[A-Za-z][A-Za-z0-9_]*(\.[A-Za-z][A-Za-z0-9_]*)+$/;
const REQUEST = /^[A-Za-z0-9._-]{1,64}$/;

const hex = (bytes) => [...new Uint8Array(bytes)].map((b) => b.toString(16).padStart(2, "0")).join("");

const answer = (value, status = 200) =>
  new Response(JSON.stringify(value), {
    status,
    headers: { "content-type": "application/json", "cache-control": "private, no-store" },
  });

const refuse = (status, error, message) => answer({ error, message }, status);

/// POST /port/key-challenge, for a signed-in person.
export async function issueKeyChallenge(request, env, user, now = Math.floor(Date.now() / 1000)) {
  let body;
  try {
    body = await request.json();
  } catch {
    return refuse(400, "bad-request", "send the package, certificate_sha256, apk_sha256 and request as JSON");
  }
  const { package: pkg, certificate_sha256: cert, apk_sha256: apk, request: req } = body || {};
  if (typeof pkg !== "string" || !PACKAGE.test(pkg) || pkg.length > 255) {
    return refuse(400, "bad-package", "that is not an Android package name");
  }
  if (typeof cert !== "string" || !HEX64.test(cert) || typeof apk !== "string" || !HEX64.test(apk)) {
    return refuse(400, "bad-digest", "certificate_sha256 and apk_sha256 are 64 lower-case hex characters");
  }
  if (typeof req !== "string" || !REQUEST.test(req)) {
    return refuse(400, "bad-request-id", "the import request is letters, digits, '.', '_' and '-', up to 64");
  }
  const nonce = hex(crypto.getRandomValues(new Uint8Array(16)));
  const statement = {
    v: 1,
    purpose: PURPOSE,
    package: pkg,
    certificate_sha256: cert,
    apk_sha256: apk,
    request: req,
    account: user.id,
    nonce,
    expires_at: now + CHALLENGE_SECONDS,
  };
  await env.APPS.put(`keychal:${nonce}`, JSON.stringify(statement), {
    expirationTtl: CHALLENGE_SECONDS + 60,
  });
  return answer({ statement });
}

/// POST /port/key-challenge/answer: the signed challenge APK as the body.
/// `call(name, bytes)` runs one validator export (index.js's callValidator).
export async function answerKeyChallenge(request, env, user, call, now = Math.floor(Date.now() / 1000)) {
  const bytes = new Uint8Array(await request.arrayBuffer());
  if (!bytes.length || bytes.length > MAX_ANSWER_BYTES) {
    return refuse(400, "bad-answer", "send the signed challenge APK, at most 256 KB");
  }
  const carried = call("krate_apk_statement", bytes);
  if (!carried.ok) return refuse(422, "not-a-challenge", carried.problem);
  const nonce = String(carried.statement.nonce || "");
  const key = `keychal:${nonce}`;
  const held = /^[0-9a-f]{32}$/.test(nonce) ? await env.APPS.get(key) : null;
  if (!held) {
    return refuse(410, "unknown-challenge", "this challenge is unknown, already answered or expired: ask for a new one");
  }
  const issued = JSON.parse(held);
  if (issued.account !== user.id) {
    return refuse(403, "not-yours", "this challenge was issued to another account");
  }
  // Single use, right or wrong: an answer cannot be retried or replayed.
  await env.APPS.delete(key);
  const meta = new TextEncoder().encode(JSON.stringify({ issued, now }));
  const frame = new Uint8Array(4 + meta.length + bytes.length);
  new DataView(frame.buffer).setUint32(0, meta.length, true);
  frame.set(meta, 4);
  frame.set(bytes, 4 + meta.length);
  const checked = call("krate_apk_check", frame);
  if (!checked.ok) return refuse(422, checked.reason || "not-proved", checked.problem);
  const attestation = {
    v: 1,
    kind: "android-signing-key-control",
    package: issued.package,
    certificate_sha256: issued.certificate_sha256,
    signed_with: checked.certificate_sha256,
    apk_sha256: issued.apk_sha256,
    request: issued.request,
    account: issued.account,
    nonce: issued.nonce,
    verified_at: now,
  };
  const signer = await leaseKey(env);
  if (!signer) return refuse(503, "no-key", "this hub has no signing key, so it cannot vouch for the proof");
  const signature = await crypto.subtle.sign(
    { name: "Ed25519" },
    signer.privateKey,
    new TextEncoder().encode(KEY_CONTROL_DOMAIN + canonicalJson(attestation)),
  );
  return answer({ attestation, signature: hex(signature), key_id: signer.keyId, alg: "Ed25519" });
}
