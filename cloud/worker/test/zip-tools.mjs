/* Small zip helpers for tests that hand-build or patch archives. The Rust
 * validator at the door verifies every entry's CRC, so a test that changes
 * bytes inside a stored entry must fix the checksum in the local header
 * and the central directory, or it is testing the checksum and not the
 * rule it meant to.
 */
import { crc32 } from "node:zlib";

const u16 = (b, i) => b[i] | (b[i + 1] << 8);
const u32 = (b, i) => (b[i] | (b[i + 1] << 8) | (b[i + 2] << 16) | (b[i + 3] << 24)) >>> 0;
const put32 = (b, i, v) => { b[i] = v & 0xff; b[i + 1] = (v >> 8) & 0xff; b[i + 2] = (v >> 16) & 0xff; b[i + 3] = (v >>> 24) & 0xff; };

/// Central-directory records: name, offsets of the record and of its local header.
export function directory(bytes) {
  let eocd = -1;
  for (let i = bytes.length - 22; i >= 0; i -= 1) {
    if (u32(bytes, i) === 0x06054b50) { eocd = i; break; }
  }
  if (eocd < 0) throw new Error("no end of central directory");
  const count = u16(bytes, eocd + 10);
  let at = u32(bytes, eocd + 16);
  const out = [];
  const dec = new TextDecoder();
  for (let n = 0; n < count; n += 1) {
    const nameLen = u16(bytes, at + 28);
    const extraLen = u16(bytes, at + 30);
    const commentLen = u16(bytes, at + 32);
    out.push({ name: dec.decode(bytes.subarray(at + 46, at + 46 + nameLen)), record: at, local: u32(bytes, at + 42), method: u16(bytes, at + 10), size: u32(bytes, at + 24) });
    at += 46 + nameLen + extraLen + commentLen;
  }
  return out;
}

/// Overwrite a STORED entry's content in place with same-length bytes and
/// fix its CRC in both headers. Returns the patched copy.
export function patchStored(bytes, name, replace) {
  const out = new Uint8Array(bytes);
  const entry = directory(out).find((e) => e.name === name);
  if (!entry) throw new Error(`no entry ${name}`);
  if (entry.method !== 0) throw new Error(`${name} is not stored`);
  const lh = entry.local;
  const start = lh + 30 + u16(out, lh + 26) + u16(out, lh + 28);
  const current = out.subarray(start, start + entry.size);
  const next = replace(new Uint8Array(current));
  if (next.length !== entry.size) throw new Error("patched content must keep its length");
  out.set(next, start);
  const crc = crc32(next) >>> 0;
  put32(out, lh + 14, crc);
  put32(out, entry.record + 16, crc);
  return out;
}

export { crc32 };
