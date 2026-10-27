/* window.KrateOrgDevice -- the dashboard's two calls (stable):
 *   ready(slug, {name}) -> {made, holds, recoveryCode}
 *       make the organisation's signing key and data key in this browser
 *       and register it as a device ("browser"); when the organisation has
 *       them already, register and do this browser's share.
 *   sign(slug, bytes)   -> Uint8Array: the .krate marked for the
 *       organisation and signed as it, through the validator module.
 *   publish(slug, bytes, {name, note, role, wait, onStep})
 *       -> {state, digest, solo, url, approvers, publisher}
 *       send an app to the organisation for approval from this tab, as
 *       `krate publish --org` does: signed as the organisation when this
 *       browser holds its signing key, otherwise with this browser's own
 *       release key, asked for and approved in the background like a
 *       developer's computer. state "waiting-for-publisher" while the
 *       organisation has not approved this browser yet (nothing was sent).
 *   duties(slug)        -> what this browser did as a holder, or null when
 *       it is not one of the organisation's devices: pass keys on, approve
 *       waiting publishers, withdraw the keys of people who left.
 *   Rejections carry .reason: "not-holder" (not an owner or admin, or this
 *   browser does not hold the signing key yet), "unsupported" (no WebCrypto
 *   Ed25519/X25519, WebAssembly or IndexedDB), or the hub's own reason.
 *   The hub and sign-in are window.KrateOrg's (kit/org.js).
 *
 * The browser as an organisation owner's first device (IC-958, the
 * one-minute rule): a company makes its organisation, signs and releases
 * its first app from this tab, and nobody sees a key.
 *
 * What runs where:
 * - Keys are made here by WebCrypto: the device's X25519 key, the
 *   organisation key (X25519, the data keyring) and the organisation's
 *   signing key (Ed25519). They must be extractable -- each is sealed to
 *   other devices of the organisation -- so the device's own key is kept in
 *   IndexedDB sealed by an AES-GCM key the browser keeps and never lets out
 *   (non-extractable). Everything else lives on the hub only as envelopes
 *   sealed to devices: this tab keeps no copy of them.
 * - Sealing (HPKE), the recovery code, permission slips and signing a
 *   bundle are done by the hub's own validator module (crates/bundle and
 *   crates/orgkeys compiled to WebAssembly, served at <hub>/validator.wasm):
 *   the same code a computer runs, not a JavaScript copy. Its randomness is
 *   crypto.getRandomValues, handed in with each call.
 * - Honest limit: clearing this site's data forgets this device. The
 *   organisation's keys are not lost while another member's device or the
 *   recovery code exists; `restore` brings them back with the code.
 *
 * window.KrateSigning (or globalThis anywhere else, for tests):
 *   loadModule(source)                -> mod  (URL, bytes or WebAssembly.Module)
 *   openDevice({store, name})         -> device
 *   indexedStore() / memoryStore()    -> where the device is kept
 *   setUp({hub, token, slug, device, mod})
 *        -> {recoveryCode, signing: true}   the keyring and the signing key
 *   release({hub, token, slug, device, mod, bytes, name, onStep})
 *        -> {digest, state, solo, url}      sign as the organisation, send,
 *                                           and release it when it may
 *   duties({hub, token, slug, device, mod})
 *        -> {keyGrants, ringGrants, approved, withdrawn}
 *   publish({hub, token, slug, device, mod, store, bytes, name, note,
 *            holder, wait, onStep})
 *        -> {state, digest, solo, url, approvers, publisher}
 *   approvePublisher({hub, token, slug, device, mod, key})
 *        -> the publisher record, signed by this tab when it can
 *   restore({hub, token, slug, device, mod, code}) -> {keyring, signing}
 *   refresh({hub, token, slug, device, mod, held})
 *        -> {status: "holds"|"waiting"|"no-key", envelope}  a member's
 *           browser and the key: what a computer does before each sync
 * Every hub refusal is thrown as an Error with .reason and .status and the
 * hub's own sentence as its message.
 */
(function (root) {
  "use strict";

  var subtle = root.crypto.subtle;
  var enc = new TextEncoder();
  var dec = new TextDecoder();

  function hex(bytes) {
    var s = "";
    for (var i = 0; i < bytes.length; i++) s += (bytes[i] + 256).toString(16).slice(1);
    return s;
  }
  function b64(bytes) {
    var s = "";
    for (var i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode.apply(null, bytes.subarray(i, i + 0x8000));
    return root.btoa(s);
  }
  function fromB64url(text) {
    var t = String(text).replace(/-/g, "+").replace(/_/g, "/");
    while (t.length % 4) t += "=";
    var raw = root.atob(t);
    var out = new Uint8Array(raw.length);
    for (var i = 0; i < raw.length; i++) out[i] = raw.charCodeAt(i);
    return out;
  }
  function entropy() {
    return b64(root.crypto.getRandomValues(new Uint8Array(512)));
  }
  function randomKey() {
    return hex(root.crypto.getRandomValues(new Uint8Array(16)));
  }

  // ------------------------------------------------------- the module

  /// The validator module, from a URL (normally <hub>/validator.wasm), its
  /// bytes, or a compiled module. It has no imports.
  async function loadModule(source) {
    var module = source;
    if (typeof source === "string") {
      var r = await root.fetch(source);
      if (!r.ok) throw new Error("Krate's signing code did not load (" + r.status + ") -- reload the page");
      module = await root.WebAssembly.compile(await r.arrayBuffer());
    } else if (!(source instanceof root.WebAssembly.Module)) {
      module = await root.WebAssembly.compile(source);
    }
    var ex = (await root.WebAssembly.instantiate(module, {})).exports;
    function raw(name, bytes) {
      var input = ex.krate_alloc(bytes.length);
      new Uint8Array(ex.memory.buffer, input, bytes.length).set(bytes);
      var out = ex[name](input, bytes.length);
      var len = new DataView(ex.memory.buffer).getUint32(out, true);
      var copy = new Uint8Array(ex.memory.buffer, out + 4, len).slice();
      ex.krate_free(out, 4 + len);
      ex.krate_free(input, bytes.length);
      return copy;
    }
    function json(name, value) {
      var answer = JSON.parse(dec.decode(raw(name, enc.encode(JSON.stringify(value)))));
      if (!answer.ok) throw new Error(answer.problem || "the signing code refused");
      return answer;
    }
    return {
      /// One keyring or signing-ring operation (crates/validator-wasm
      /// browser.rs); entropy is added here.
      keys: function (op) {
        var withEntropy = Object.assign({ entropy: entropy() }, op);
        return json("krate_keys", withEntropy);
      },
      /// Mark `bytes` for the organisation and sign them: {meta, bytes}.
      sign: function (header, bytes) {
        var head = enc.encode(JSON.stringify(header));
        var frame = new Uint8Array(4 + head.length + bytes.length);
        new DataView(frame.buffer).setUint32(0, head.length, true);
        frame.set(head, 4);
        frame.set(bytes, 4 + head.length);
        var out = raw("krate_sign", frame);
        var n = new DataView(out.buffer, out.byteOffset).getUint32(0, true);
        var meta = JSON.parse(dec.decode(out.subarray(4, 4 + n)));
        if (!meta.ok) throw new Error(meta.problem || "the app could not be signed");
        return { meta: meta, bytes: out.slice(4 + n) };
      },
      judge: function (bytes) {
        return JSON.parse(dec.decode(raw("krate_judge", bytes)));
      },
      /// One team-data call (crates/validator-wasm teamdata.rs): the
      /// device's copy of an app's data, its merge and its sealing. Answers
      /// the module's JSON as it is, {ok, ...} or {ok:false, error}: an
      /// error here is the app's to see. `random` bytes of entropy are added
      /// for a call that seals.
      data: function (op, random) {
        var input = random ? Object.assign({ entropy: b64(root.crypto.getRandomValues(new Uint8Array(random))) }, op) : op;
        return JSON.parse(dec.decode(raw("krate_org_data", enc.encode(JSON.stringify(input)))));
      },
    };
  }

  // ----------------------------------------------------- the device

  /// Where the device is kept: IndexedDB, one database for Krate.
  function indexedStore(name) {
    name = name || "krate-org";
    function db() {
      return new Promise(function (ok, fail) {
        var r = root.indexedDB.open(name, 1);
        r.onupgradeneeded = function () { r.result.createObjectStore("kv"); };
        r.onsuccess = function () { ok(r.result); };
        r.onerror = function () { fail(r.error); };
      });
    }
    function tx(mode, fn) {
      return db().then(function (d) {
        return new Promise(function (ok, fail) {
          var t = d.transaction("kv", mode);
          var req = fn(t.objectStore("kv"));
          t.oncomplete = function () { ok(req && req.result); };
          t.onerror = function () { fail(t.error); };
        });
      });
    }
    return {
      get: function (k) { return tx("readonly", function (s) { return s.get(k); }); },
      put: function (k, v) { return tx("readwrite", function (s) { return s.put(v, k); }); },
      del: function (k) { return tx("readwrite", function (s) { return s.delete(k); }); },
    };
  }

  /// The same, in memory (tests, and a browser that refuses IndexedDB).
  function memoryStore() {
    var m = new Map();
    return {
      get: async function (k) { return m.get(k); },
      put: async function (k, v) { m.set(k, v); },
      del: async function (k) { m.delete(k); },
    };
  }

  /// This browser's device key: made once, kept sealed by a key the
  /// browser keeps. `name` is what the dashboard lists it as.
  async function openDevice(opts) {
    opts = opts || {};
    var store = opts.store || indexedStore();
    var rec = await store.get("device");
    if (!rec) {
      var pair = await subtle.generateKey({ name: "X25519" }, true, ["deriveBits"]);
      var jwk = await subtle.exportKey("jwk", pair.privateKey);
      var wrap = await subtle.generateKey({ name: "AES-GCM", length: 256 }, false, ["encrypt", "decrypt"]);
      var iv = root.crypto.getRandomValues(new Uint8Array(12));
      var secret = hex(fromB64url(jwk.d));
      var sealed = await subtle.encrypt({ name: "AES-GCM", iv: iv }, wrap, enc.encode(secret));
      rec = { v: 1, public_key: hex(fromB64url(jwk.x)), wrap: wrap, iv: iv, sealed: new Uint8Array(sealed), name: opts.name || browserName() };
      await store.put("device", rec);
    }
    return {
      public_key: rec.public_key,
      name: rec.name,
      secret: async function () {
        return dec.decode(await subtle.decrypt({ name: "AES-GCM", iv: rec.iv }, rec.wrap, rec.sealed));
      },
    };
  }

  // -------------------------------------------------------- the hub

  function client(hub, token, slug) {
    async function call(method, path, body, extra) {
      var headers = { accept: "application/json", authorization: "Bearer " + token };
      if (method !== "GET") headers["idempotency-key"] = randomKey();
      var init = { method: method, headers: headers };
      if (body instanceof Uint8Array) {
        init.body = body;
        Object.assign(headers, extra || {});
      } else if (body !== undefined) {
        headers["content-type"] = "application/json";
        init.body = JSON.stringify(body);
      }
      var r;
      try {
        r = await root.fetch(hub.replace(/\/$/, "") + "/orgs/" + slug + path, init);
      } catch (e) {
        var off = new Error("could not reach Krate -- check the connection and try again");
        off.reason = "offline";
        throw off;
      }
      var text = await r.text();
      var value = null;
      try { value = text ? JSON.parse(text) : null; } catch (e) { value = null; }
      if (!r.ok) {
        var err = new Error((value && value.message) || "Krate said no (" + r.status + ")");
        err.reason = (value && value.error) || "refused";
        err.status = r.status;
        throw err;
      }
      return value;
    }
    return call;
  }

  async function deviceIdOf(mod, device) {
    return mod.keys({ op: "device_id", public_key: device.public_key }).device_id;
  }

  async function register(call, mod, device) {
    var id = await deviceIdOf(mod, device);
    await call("PUT", "/keys/devices/" + id, { public_key: device.public_key, name: device.name, kind: "browser" });
    return id;
  }

  async function x25519Secret() {
    var pair = await subtle.generateKey({ name: "X25519" }, true, ["deriveBits"]);
    return hex(fromB64url((await subtle.exportKey("jwk", pair.privateKey)).d));
  }

  /// Make the organisation's keyring and signing key in this browser, as
  /// its first owner device. Answers the recovery code, to be shown once.
  async function setUp(o) {
    var call = client(o.hub, o.token, o.slug);
    var mod = o.mod;
    var id = await register(call, mod, o.device);
    // The keyring, unless the organisation has one already (made by Krate
    // on a computer); then its recovery key is the one the ring seals to.
    var keys = await call("GET", "/keys?device=" + id);
    var made = null;
    var recoveryPublic = keys.recovery ? keys.recovery.public_key : null;
    if (!keys.current) {
      made = mod.keys({ op: "keyring_create", org: o.slug, secret: await x25519Secret(), device_public: o.device.public_key });
      await call("POST", "/keys/init", { device_id: id, public_key: made.public_key, envelope: made.envelope, recovery: made.recovery });
      recoveryPublic = made.recovery.public_key;
    } else if (!recoveryPublic) {
      var sv = await call("GET", "/signing?device=" + id);
      recoveryPublic = sv.recovery ? sv.recovery.public_key : null;
    }
    var signing = await subtle.generateKey({ name: "Ed25519" }, true, ["sign", "verify"]);
    var jwk = await subtle.exportKey("jwk", signing.privateKey);
    var ring = mod.keys({
      op: "ring_create", org: o.slug, seed: hex(fromB64url(jwk.d)), public: hex(fromB64url(jwk.x)),
      device_public: o.device.public_key, recovery_public: recoveryPublic || undefined,
    });
    jwk = null;
    await call("POST", "/signing/init", { device_id: id, version: 1, public_key: ring.public_key, envelope: ring.envelope, recovery_blob: ring.recovery_blob });
    return { recoveryCode: made ? made.recovery_code : null, signing: true, deviceId: id };
  }

  async function ringOf(call, mod, device) {
    var id = await register(call, mod, device);
    var view = await call("GET", "/signing?device=" + id);
    return { id: id, view: view };
  }

  /// Sign `bytes` as the organisation in this tab, send them, and release
  /// them when the organisation lets the sender release alone. `onStep`
  /// hears each step in words, for the page to show.
  async function release(o) {
    var say = o.onStep || function () {};
    var call = client(o.hub, o.token, o.slug);
    var mod = o.mod;
    var r = await ringOf(call, mod, o.device);
    if (!r.view.envelope) {
      var e = new Error(r.view.ring ? "this browser cannot sign for the organisation yet -- another owner's computer passes it on the next time it runs Krate" : "the organisation cannot sign apps yet");
      e.reason = r.view.ring ? "signing-key-not-held" : "no-signing-key";
      throw e;
    }
    say("Signing " + (o.name || "the app") + "...");
    var secret = await o.device.secret();
    var signed = mod.sign(
      { org: o.slug, signed_at: Math.floor(Date.now() / 1000), ring: { envelope: r.view.envelope.blob, device_secret: secret } },
      o.bytes,
    );
    secret = null;
    say("Sending it...");
    var headers = { "x-krate-encoding": "uri" };
    if (o.name) headers["x-krate-name"] = encodeURIComponent(o.name);
    var sent = await call("POST", "/candidates", signed.bytes, headers);
    var out = { digest: sent.digest || signed.meta.digest, state: sent.state, solo: false, url: null, approvers: sent.approvers || [] };
    if (sent.solo_release) {
      say("Releasing it...");
      var done = await call("POST", "/candidates/" + out.digest + "/approve", { accept: [] });
      out.state = "released";
      out.solo = Boolean(done.release && done.release.solo);
      out.url = done.release && done.release.url;
    }
    return out;
  }

  // ------------------------------------------ publishing from this tab

  /// This browser's release key for one organisation: an Ed25519 key made
  /// here the first time it publishes there, kept sealed in the store by a
  /// key the browser never lets out, as the device key is. The hub only
  /// ever sees its public half, and the permission slip a holder signs.
  async function releaseKeyOf(store, slug) {
    var name = "release:" + slug;
    var rec = await store.get(name);
    if (!rec) {
      var pair = await subtle.generateKey({ name: "Ed25519" }, true, ["sign", "verify"]);
      var jwk = await subtle.exportKey("jwk", pair.privateKey);
      var wrap = await subtle.generateKey({ name: "AES-GCM", length: 256 }, false, ["encrypt", "decrypt"]);
      var iv = root.crypto.getRandomValues(new Uint8Array(12));
      var sealed = await subtle.encrypt({ name: "AES-GCM", iv: iv }, wrap, enc.encode(hex(fromB64url(jwk.d))));
      rec = { v: 1, public_key: hex(fromB64url(jwk.x)), wrap: wrap, iv: iv, sealed: new Uint8Array(sealed) };
      jwk = null;
      await store.put(name, rec);
    }
    return {
      public_key: rec.public_key,
      seed: async function () {
        return dec.decode(await subtle.decrypt({ name: "AES-GCM", iv: rec.iv }, rec.wrap, rec.sealed));
      },
      forget: function () { return store.del(name); },
    };
  }

  var GONE = { revoked: 1, "revoke-due": 1, closed: 1 };
  function pause(ms) { return new Promise(function (ok) { root.setTimeout(ok, ms); }); }

  /// Send `bytes` to the organisation for approval from this tab, the way
  /// `krate publish --org` does from a computer:
  /// - signed as the organisation when `holder` (an owner or admin) and
  ///   this browser holds its signing key;
  /// - otherwise signed with this browser's release key: asked for at
  ///   POST /publishers, approved in the background by any holder device
  ///   (or by an owner's click when the organisation approves publishers by
  ///   hand), waited on for `wait` ms (6 s by default). Not approved yet,
  ///   it answers state "waiting-for-publisher" and sends nothing;
  /// - marked as the organisation's private app either way
  ///   (ext/krate/org/private.json, by the validator module), sent with its
  ///   name and release note (x-krate-name, x-krate-note), and released at
  ///   once when the hub says its sender may release it alone.
  async function publish(o) {
    var say = o.onStep || function () {};
    var call = client(o.hub, o.token, o.slug);
    var mod = o.mod;
    var bytes = o.bytes instanceof Uint8Array ? o.bytes : new Uint8Array(o.bytes);
    var now = function () { return Math.floor(Date.now() / 1000); };
    var signed = null;
    if (o.holder && o.device) {
      var r = await ringOf(call, mod, o.device);
      if (r.view.envelope && r.view.ring && r.view.ring.current) {
        say("Signing " + (o.name || "the app") + "...");
        var secret = await o.device.secret();
        signed = mod.sign({ org: o.slug, signed_at: now(), ring: { envelope: r.view.envelope.blob, device_secret: secret } }, bytes);
        secret = null;
      }
    }
    if (!signed) {
      var key = await releaseKeyOf(o.store, o.slug);
      say("Getting you ready to publish...");
      var p = await call("POST", "/publishers", { public_key: key.public_key, name: (o.device && o.device.name) || browserName() });
      var until = Date.now() + (o.wait === undefined ? 6000 : o.wait);
      while (p.state !== "signed" && !GONE[p.state] && Date.now() < until) {
        await pause(Math.max(50, Math.min(1500, until - Date.now())));
        p = await call("GET", "/publishers/" + key.public_key);
      }
      if (GONE[p.state]) {
        // This browser's permission was withdrawn: the next send makes a
        // new key and asks again.
        await key.forget();
        var gone = new Error("this browser's earlier permission to publish was withdrawn, so nothing was sent -- send it again to ask afresh");
        gone.reason = "publisher-withdrawn";
        throw gone;
      }
      if (p.state !== "signed") return { state: "waiting-for-publisher", publisher: p };
      var slip = p.delegation;
      if (!slip || !slip.delegation || String(slip.delegation.key).toLowerCase() !== key.public_key) {
        var odd = new Error("the organisation's approval is for another key -- send it again");
        odd.reason = "bad-delegation";
        throw odd;
      }
      say("Signing " + (o.name || "the app") + "...");
      var seed = await key.seed();
      signed = mod.sign({ org: o.slug, signed_at: now(), seed: seed, public: key.public_key, delegation: slip }, bytes);
      seed = null;
    }
    say("Sending it...");
    var headers = { "x-krate-encoding": "uri" };
    if (o.name) headers["x-krate-name"] = encodeURIComponent(o.name);
    var note = String(o.note || "").trim();
    if (note) headers["x-krate-note"] = encodeURIComponent(note);
    var sent = await call("POST", "/candidates", signed.bytes, headers);
    var out = { state: sent.state, digest: sent.digest || signed.meta.digest, solo: false, url: null, approvers: sent.approvers || [], signer: signed.meta.signer };
    if (sent.solo_release) {
      say("Releasing it...");
      var done = await call("POST", "/candidates/" + out.digest + "/approve", { accept: [] });
      out.state = "released";
      out.solo = Boolean(done.release && done.release.solo);
      out.url = done.release && done.release.url;
    }
    return out;
  }

  /// A holder's duties, from this tab: pass the keyring and the signing key
  /// to new devices, approve waiting publishers, withdraw the release keys
  /// of people who left. What a computer running Krate does in the
  /// background.
  async function duties(o) {
    var call = client(o.hub, o.token, o.slug);
    var mod = o.mod;
    var did = { keyGrants: 0, ringGrants: 0, approved: [], withdrawn: 0 };
    var id = await register(call, mod, o.device);
    var secret = await o.device.secret();
    var keys = await call("GET", "/keys?device=" + id);
    if (keys.envelope && keys.current && keys.envelope.version === keys.current.version && keys.current.state !== "stale") {
      for (var i = 0; i < (keys.requests || []).length; i++) {
        var q = keys.requests[i];
        if ((await deviceIdOf(mod, { public_key: q.public_key })) !== q.device_id) continue;
        var g = mod.keys({ op: "keyring_grant", org: o.slug, envelope: keys.envelope.blob, device_secret: secret, to_public: q.public_key });
        await call("POST", "/keys/grants", { device_id: q.device_id, version: g.version, blob: g.blob, by_device: id });
        did.keyGrants += 1;
      }
    } else if (keys.envelope && keys.current && keys.current.state === "stale" && (keys.seal_to || []).length) {
      var rot = mod.keys({
        op: "keyring_rotate", org: o.slug, envelope: keys.envelope.blob, device_secret: secret, secret: await x25519Secret(),
        to: keys.seal_to, recovery_public: keys.recovery && keys.recovery.public_key,
        credentials: keys.credentials || [],
      });
      // The company connections' credentials go with it, sealed again to the
      // new key: the hub refuses a renewal that leaves one on the old key.
      await call("POST", "/keys/rotate", { version: rot.version, public_key: rot.public_key, by_device: id, envelopes: rot.envelopes, recovery_blob: rot.recovery_blob, credentials: rot.credentials || {} });
    }
    var view = await call("GET", "/signing?device=" + id);
    if (view.envelope && view.ring && view.ring.current) {
      var ring = { envelope: view.envelope.blob, device_secret: secret };
      for (var j = 0; j < view.ring_requests.length; j++) {
        var d = view.ring_requests[j];
        if ((await deviceIdOf(mod, { public_key: d.public_key })) !== d.device_id) continue;
        var rg = mod.keys(Object.assign({ op: "ring_grant", org: o.slug, to_public: d.public_key }, ring));
        await call("POST", "/signing/grants", { device_id: d.device_id, version: rg.version, blob: rg.blob, by_device: id });
        did.ringGrants += 1;
      }
      var now = Math.floor(Date.now() / 1000);
      for (var k = 0; k < view.publisher_requests.length; k++) {
        var p = view.publisher_requests[k];
        var slip = mod.keys(Object.assign({ op: "ring_delegate", org: o.slug, key: p.key, at: now }, ring));
        try {
          await call("POST", "/publishers/" + p.key + "/delegation", { delegation: slip.delegation, by_device: id });
          did.approved.push(p.display);
        } catch (e) {
          if (e.reason === "offline") throw e;
        }
      }
      if (view.revoke.length) {
        var list = mod.keys(Object.assign({ op: "ring_revoke", org: o.slug, at: now, revocations: view.revoke }, ring));
        await call("POST", "/signing/revocations", { list: list.list, by_device: id });
        did.withdrawn = view.revoke.length;
      }
    }
    secret = null;
    return did;
  }

  /// Where this browser stands with the organisation's key, as a member's
  /// device -- what a computer's Krate does before every sync
  /// (krate_orgkeys::agent::Agent::refresh): register this browser if the
  /// hub does not know it, take the keyring from its envelope (opened here,
  /// by the module), check that the hub's current public key is the one
  /// held, and pass the key on to the organisation's waiting devices.
  /// `held` is the envelope this browser kept from last time. Answers
  /// {status: "holds", envelope, version}, {status: "waiting"} (another
  /// member's device has not passed it yet, or a newer version is coming)
  /// or {status: "no-key"} (the organisation has no key yet). A new key
  /// after someone leaves is made by a computer or an owner's browser, not
  /// here.
  async function refresh(o) {
    var call = client(o.hub, o.token, o.slug);
    var mod = o.mod;
    var id = await deviceIdOf(mod, o.device);
    var view = await call("GET", "/keys?device=" + id);
    if (!view.device) {
      await call("PUT", "/keys/devices/" + id, { public_key: o.device.public_key, name: o.device.name, kind: "browser" });
      view = await call("GET", "/keys?device=" + id);
    }
    if (!view.current) return { status: "no-key" };
    var secret = await o.device.secret();
    var held = o.held || null;
    var blob = view.envelope && view.envelope.blob;
    if (blob && blob !== held) {
      // Throws when it does not open: an envelope for another key.
      mod.keys({ op: "keyring_open", org: o.slug, envelope: blob, device_secret: secret });
      held = blob;
    }
    if (!held) return { status: "waiting" };
    var opened = mod.keys({ op: "keyring_open", org: o.slug, envelope: held, device_secret: secret });
    if (opened.version < view.current.version) return { status: "waiting", envelope: held };
    if (opened.version !== view.current.version || opened.public_key !== view.current.public_key) {
      var odd = new Error("the organisation key the hub names is not the one this browser holds");
      odd.reason = "key-mismatch";
      throw odd;
    }
    if (view.current.state !== "stale") {
      for (var i = 0; i < (view.requests || []).length; i++) {
        var q = view.requests[i];
        if (q.device_id === id) continue;
        if ((await deviceIdOf(mod, { public_key: q.public_key })) !== q.device_id) continue;
        var g = mod.keys({ op: "keyring_grant", org: o.slug, envelope: held, device_secret: secret, to_public: q.public_key });
        try {
          await call("POST", "/keys/grants", { device_id: q.device_id, version: g.version, blob: g.blob, by_device: id });
        } catch (e) {
          if (e.reason === "offline") throw e;
        }
      }
    }
    secret = null;
    return { status: "holds", envelope: held, version: opened.version };
  }

  /// "Approve" on a waiting publisher, when the organisation approves new
  /// publishers by hand: recorded, then signed from this tab at once when
  /// this browser holds the signing key (otherwise the next owner's
  /// computer that runs Krate signs it).
  async function approvePublisher(o) {
    var call = client(o.hub, o.token, o.slug);
    await call("POST", "/publishers/" + o.key + "/approve", {});
    await duties(o);
    return call("GET", "/publishers/" + o.key);
  }

  /// A browser whose data was cleared, or a new one: the organisation's
  /// keyring and signing key back on this device from the recovery code.
  async function restore(o) {
    var call = client(o.hub, o.token, o.slug);
    var mod = o.mod;
    var id = await register(call, mod, o.device);
    var rec = await call("GET", "/keys/recovery");
    var base = { org: o.slug, code: o.code, params: rec.params, recovery_public: rec.public_key, device_public: o.device.public_key };
    var k = mod.keys(Object.assign({ op: "keyring_recover", blob: rec.blob }, base));
    await call("POST", "/keys/grants", { device_id: id, version: k.version, blob: k.envelope, by_device: "recovery" });
    var signing = false;
    try {
      var sr = await call("GET", "/signing/recovery");
      var s = mod.keys(Object.assign({ op: "ring_recover", blob: sr.blob }, base));
      await call("POST", "/signing/grants", { device_id: id, version: s.version, blob: s.envelope, by_device: "recovery" });
      signing = true;
    } catch (e) {
      if (e.reason === "offline") throw e;
    }
    return { keyring: true, signing: signing };
  }

  // ------------------------------------------- the dashboard's facade

  var cfg = {};
  var cached = { mod: null, device: null, store: null };

  function refuse(reason, message) {
    var e = new Error(message);
    e.reason = reason;
    return e;
  }

  async function supported() {
    if (!subtle || !root.WebAssembly || (!cfg.store && !root.indexedDB)) return false;
    try {
      await subtle.generateKey({ name: "Ed25519" }, false, ["sign", "verify"]);
      await subtle.generateKey({ name: "X25519" }, false, ["deriveBits"]);
      return true;
    } catch (e) {
      return false;
    }
  }

  function hubOf() {
    if (cfg.hub) return cfg.hub;
    var K = root.KrateOrg;
    return K && K.hub ? K.hub() : "https://hub.krate.tech";
  }

  function tokenOf() {
    if (cfg.token) return typeof cfg.token === "function" ? cfg.token() : cfg.token;
    var K = root.KrateOrg;
    return K && K.token ? K.token() : "";
  }

  /// What a person calls this browser when they see it in a list of the
  /// organisation's computers: "Chrome on Mac", "Safari on iPhone".
  function browserName() {
    var nav = typeof navigator !== "undefined" ? navigator : {};
    var ua = nav.userAgent || "";
    var b = nav.brave ? "Brave" : /Edg\//.test(ua) ? "Edge" : /Firefox\//.test(ua) ? "Firefox" : /OPR\//.test(ua) ? "Opera"
      : /Chrome\//.test(ua) ? "Chrome" : /Safari\//.test(ua) ? "Safari" : "This browser";
    var o = /iPhone/.test(ua) ? "iPhone" : /iPad/.test(ua) ? "iPad" : /Android/.test(ua) ? "Android"
      : /Macintosh|Mac OS X/.test(ua) ? "Mac" : /Windows/.test(ua) ? "Windows" : /CrOS/.test(ua) ? "Chromebook" : /Linux/.test(ua) ? "Linux" : "";
    return b + (o && b !== "This browser" ? " on " + o : "");
  }

  async function parts(name) {
    if (!(await supported())) {
      throw refuse("unsupported", "this browser cannot keep the organisation's keys -- use a current Chrome, Edge, Safari or Firefox, or Krate on a computer");
    }
    if (!cached.mod) cached.mod = await loadModule(hubOf() + "/validator.wasm");
    if (!cached.device) cached.device = await openDevice({ store: storeOf(), name: name || browserName() });
    return { hub: hubOf(), token: tokenOf(), mod: cached.mod, device: cached.device };
  }

  /// Make the organisation's signing key and data key in this browser and
  /// register it as a device -- or, when the organisation has them already,
  /// register it and do its share. Answers {made, holds, recoveryCode}.
  async function ready(slug, opts) {
    var p = await parts(opts && opts.name);
    var o = Object.assign({ slug: slug }, p);
    var call = client(o.hub, o.token, slug);
    var r = await ringOf(call, o.mod, o.device);
    if (!r.view.may_hold) throw refuse("not-holder", "only the organisation's owners and admins keep its keys in their browser");
    if (!r.view.ring) {
      var made = await setUp(o);
      return { made: true, holds: true, recoveryCode: made.recoveryCode };
    }
    await duties(o);
    var again = await call("GET", "/signing?device=" + r.id);
    return { made: false, holds: Boolean(again.envelope && again.ring && again.ring.current), recoveryCode: null };
  }

  /// Mark a .krate for the organisation and sign it as the organisation,
  /// through the validator module. Answers the signed bytes.
  async function sign(slug, bytes) {
    var p = await parts();
    var call = client(p.hub, p.token, slug);
    var r = await ringOf(call, p.mod, p.device);
    if (!r.view.envelope || !r.view.ring || !r.view.ring.current) {
      throw refuse("not-holder", "this browser cannot sign for the organisation yet -- another owner's device passes it on the next time it runs Krate");
    }
    var secret = await p.device.secret();
    var signed = p.mod.sign({ org: slug, signed_at: Math.floor(Date.now() / 1000), ring: { envelope: r.view.envelope.blob, device_secret: secret } }, bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes));
    return signed.bytes;
  }

  function storeOf() {
    if (!cached.store) cached.store = cfg.store || indexedStore();
    return cached.store;
  }

  /// Send a .krate to the organisation for approval from this tab (see
  /// KrateSigning.publish). `role` is the person's role there: an owner or
  /// admin may sign as the organisation when this browser holds its key.
  async function publishFacade(slug, bytes, opts) {
    opts = opts || {};
    if (!(await supported())) {
      throw refuse("unsupported", "this browser cannot sign apps -- use a current Chrome, Edge, Safari or Firefox, or Krate Studio on a computer");
    }
    if (!cached.mod) cached.mod = await loadModule(hubOf() + "/validator.wasm");
    if (!cached.device) cached.device = await openDevice({ store: storeOf(), name: browserName() });
    return publish({
      hub: hubOf(), token: tokenOf(), slug: slug, mod: cached.mod, device: cached.device, store: storeOf(),
      bytes: bytes, name: opts.name, note: opts.note, wait: opts.wait, onStep: opts.onStep,
      holder: opts.role === "owner" || opts.role === "admin",
    });
  }

  /// A holder's duties from this tab, quietly: only when this browser is
  /// already one of the organisation's devices (it set the organisation up
  /// or released from here), and only for an owner or admin. null when it
  /// is not; nothing is made or registered for a browser that never was.
  async function quietDuties(slug) {
    if (!(await supported())) return null;
    if (!(await storeOf().get("device"))) return null;
    var p = await parts();
    var o = Object.assign({ slug: slug }, p);
    var call = client(o.hub, o.token, slug);
    var r = await ringOf(call, o.mod, o.device);
    if (!r.view.may_hold || !r.view.envelope) return null;
    return duties(o);
  }

  root.KrateOrgDevice = {
    ready: ready,
    sign: sign,
    publish: publishFacade,
    duties: quietDuties,
    /// For a page on a development hub, and for tests: {hub, token, store}.
    configure: function (c) {
      cfg = Object.assign({}, c);
      cached = { mod: null, device: null, store: null };
    },
  };

  root.KrateSigning = {
    loadModule: loadModule,
    openDevice: openDevice,
    indexedStore: indexedStore,
    memoryStore: memoryStore,
    setUp: setUp,
    release: release,
    duties: duties,
    publish: publish,
    approvePublisher: approvePublisher,
    restore: restore,
    refresh: refresh,
  };
})(typeof window !== "undefined" ? window : globalThis);
