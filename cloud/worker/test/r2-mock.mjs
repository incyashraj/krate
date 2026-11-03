/* An R2 bucket the way the worker actually uses one: put with a sha256 it
 * must match, head/get answering size and checksums, delete. Shared by the
 * tests that drive the real publish handler, because publish now reads its
 * object back and a mock that answers `{}` would fail every publish
 * (IC-833, tests 1837 and 1840).
 */
export async function sha256Hex(bytes) {
  const digest = await crypto.subtle.digest("SHA-256", bytes);
  return [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, "0")).join("");
}

export function r2Mock(blobs = new Map()) {
  const uploads = new Map();
  const object = async (k) => {
    const v = blobs.get(k);
    if (!v) return null;
    const meta = blobs.get(`${k}\u0000meta`) || {};
    return {
      key: k,
      size: v.byteLength,
      checksums: { sha256: meta.sha256 === false ? undefined : await crypto.subtle.digest("SHA-256", v) },
      customMetadata: meta.customMetadata || {},
      body: v,
      arrayBuffer: async () => v,
    };
  };
  return {
    // This machine's stand-in, never R2 itself (publish-corpus.test.mjs, 1823).
    __standIn: true,
    head: (k) => object(k),
    get: (k) => object(k),
    put: async (k, v, opts = {}) => {
      const bytes = v instanceof Uint8Array ? v : new Uint8Array(v);
      // R2 refuses bytes whose digest is not the one the caller named.
      if (opts.sha256 && opts.sha256 !== (await sha256Hex(bytes))) {
        throw new Error("R2: the sha256 checksum did not match");
      }
      blobs.set(k, bytes);
      if (opts.customMetadata) blobs.set(`${k}\u0000meta`, { customMetadata: { ...opts.customMetadata } });
      else blobs.delete(`${k}\u0000meta`);
      return object(k);
    },
    delete: async (k) => {
      blobs.delete(k);
    },
    list: async ({ prefix = "" } = {}) => ({
      objects: await Promise.all(
        [...blobs.keys()].filter((k) => !k.includes("\u0000") && k.startsWith(prefix)).map((k) => object(k)),
      ),
      truncated: false,
    }),
    // Multipart, held to R2's own rules (IC-945): every part but the last
    // the same size and at least 5 MiB, completed only with the etags
    // uploadPart returned.
    createMultipartUpload: async (key, opts = {}) => {
      const uploadId = `u${uploads.size + 1}`;
      uploads.set(uploadId, { key, parts: new Map(), customMetadata: opts.customMetadata });
      return multipart(key, uploadId);
    },
    resumeMultipartUpload: (key, uploadId) => multipart(key, uploadId),
    _blobs: blobs,
    _uploads: uploads,
  };

  function multipart(key, uploadId) {
    return {
      key,
      uploadId,
      uploadPart: async (n, value) => {
        const up = uploads.get(uploadId);
        if (!up || up.key !== key) throw new Error("R2: NoSuchUpload");
        const bytes = await bytesOf(value);
        const etag = `e${n}-${bytes.byteLength}-${(await sha256Hex(bytes)).slice(0, 8)}`;
        up.parts.set(n, { bytes, etag });
        return { partNumber: n, etag };
      },
      abort: async () => {
        uploads.delete(uploadId);
      },
      complete: async (parts) => {
        const up = uploads.get(uploadId);
        if (!up || up.key !== key) throw new Error("R2: NoSuchUpload");
        const chosen = parts.map((p) => {
          const got = up.parts.get(p.partNumber);
          if (!got || got.etag !== p.etag) throw new Error("R2: InvalidPart");
          return got.bytes;
        });
        for (let i = 0; i < chosen.length - 1; i++) {
          if (chosen[i].byteLength < 5 * 1024 * 1024) throw new Error("R2: EntityTooSmall");
          if (chosen[i].byteLength !== chosen[0].byteLength) {
            throw new Error("R2: All non-trailing parts must have the same size");
          }
        }
        const total = chosen.reduce((n, b) => n + b.byteLength, 0);
        const whole = new Uint8Array(total);
        let at = 0;
        for (const b of chosen) {
          whole.set(b, at);
          at += b.byteLength;
        }
        blobs.set(key, whole);
        // A multipart object carries no whole-object sha256 in R2.
        blobs.set(`${key}\u0000meta`, { sha256: false, customMetadata: up.customMetadata ? { ...up.customMetadata } : undefined });
        uploads.delete(uploadId);
        return object(key);
      },
    };
  }
}

async function bytesOf(value) {
  if (value instanceof Uint8Array) return value;
  if (value instanceof ArrayBuffer) return new Uint8Array(value);
  if (value && typeof value.getReader === "function") {
    const reader = value.getReader();
    const chunks = [];
    let n = 0;
    for (;;) {
      const { done, value: chunk } = await reader.read();
      if (done) break;
      chunks.push(chunk);
      n += chunk.byteLength;
    }
    const out = new Uint8Array(n);
    let at = 0;
    for (const c of chunks) {
      out.set(c, at);
      at += c.byteLength;
    }
    return out;
  }
  return new TextEncoder().encode(String(value));
}
