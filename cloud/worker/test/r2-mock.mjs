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
  const object = async (k) => {
    const v = blobs.get(k);
    if (!v) return null;
    const meta = blobs.get(`${k}\u0000meta`) || {};
    return {
      key: k,
      size: v.byteLength,
      checksums: { sha256: meta.sha256 === false ? undefined : await crypto.subtle.digest("SHA-256", v) },
      body: v,
      arrayBuffer: async () => v,
    };
  };
  return {
    head: (k) => object(k),
    get: (k) => object(k),
    put: async (k, v, opts = {}) => {
      const bytes = v instanceof Uint8Array ? v : new Uint8Array(v);
      // R2 refuses bytes whose digest is not the one the caller named.
      if (opts.sha256 && opts.sha256 !== (await sha256Hex(bytes))) {
        throw new Error("R2: the sha256 checksum did not match");
      }
      blobs.set(k, bytes);
      blobs.delete(`${k}\u0000meta`);
      return object(k);
    },
    delete: async (k) => {
      blobs.delete(k);
    },
    _blobs: blobs,
  };
}
