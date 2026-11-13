// What changed in an app's code, for the person approving it (G15).
//
// Every .krate an organisation is sent carries its source under `source/`.
// This compares a waiting version's source with the source of the release
// it would replace, file by file, and says what a reviewer most needs to
// look at -- without any AI: the same bytes always give the same answer.
//
//   files   each source file added, removed or changed, with a line diff
//           for a text file (unified, 3 lines of context), bounded: at most
//           FILE_DIFF_MAX characters of diff per file and TOTAL_DIFF_MAX in
//           all. A file past either bound says so (`truncated`, `omitted`);
//           nothing is cut silently.
//   risks   plain rows, each computed from the bytes: new permissions and
//           network hosts, new or changed dependencies (Cargo.toml and
//           Cargo.lock), new `unsafe` code, new binary files, how much the
//           compiled app grew, and how the app was made.
//
// Change is found from the zip's own directory (each entry's CRC-32 and
// size), so an unchanged file is never inflated. Only changed text files
// are, each up to READ_MAX bytes, at most INFLATE_BUDGET bytes per side.
// The bundle is read once when it is small and by ranges when it is big.

export const CHANGES_VERSION = 1;
export const TOTAL_DIFF_MAX = 400 * 1024;
export const FILE_DIFF_MAX = 64 * 1024;
export const READ_MAX = 1024 * 1024;
export const INFLATE_BUDGET = 16 * 1024 * 1024;
const WHOLE_READ_MAX = 24 * 1024 * 1024;
const LINE_MAX = 1000;
/// Below this much room left in the total bound, a file's diff is left out.
const OMIT_UNDER = 2048;
const CONTEXT = 3;
const MAX_EDITS = 1000;
const FILES_MAX = 2000;
const DEP_ROWS_MAX = 30;
const SOURCE = "source/";

const u16 = (b, i) => b[i] | (b[i + 1] << 8);
const u32 = (b, i) => (b[i] | (b[i + 1] << 8) | (b[i + 2] << 16) | (b[i + 3] << 24)) >>> 0;

/// A reader over bytes in memory: `{size, read(offset, length)}`.
export function bytesReader(bytes) {
  return { size: bytes.length, read: async (offset, length) => bytes.subarray(offset, offset + length) };
}

/// A reader over an R2 object: the whole object once when it is small,
/// by ranges when it is big. `get(key, options)` is the bucket's.
export async function bucketReader(bucket, key) {
  const head = await bucket.head(key);
  if (!head) return null;
  const size = head.size;
  if (size <= WHOLE_READ_MAX) {
    const object = await bucket.get(key);
    if (!object) return null;
    return bytesReader(new Uint8Array(await object.arrayBuffer()));
  }
  return {
    size,
    async read(offset, length) {
      const object = await bucket.get(key, { range: { offset, length } });
      if (!object) throw new Error("the stored app could not be read");
      const buf = new Uint8Array(await object.arrayBuffer());
      // A store that ignores the range answers the whole object.
      return buf.length === length ? buf : buf.subarray(offset, offset + length);
    },
  };
}

/// The zip's central directory: `[{name, method, crc, compressed, size, local}]`,
/// or null when there is none to read.
export async function zipListing(reader) {
  const tailLength = Math.min(reader.size, 22 + 65535);
  const tail = await reader.read(reader.size - tailLength, tailLength);
  let eocd = -1;
  for (let i = tail.length - 22; i >= 0; i -= 1) {
    if (u32(tail, i) === 0x06054b50) {
      eocd = i;
      break;
    }
  }
  if (eocd < 0) return null;
  const count = u16(tail, eocd + 10);
  const dirSize = u32(tail, eocd + 12);
  const dirAt = u32(tail, eocd + 16);
  if (dirAt + dirSize > reader.size) return null;
  const dir = await reader.read(dirAt, dirSize);
  const dec = new TextDecoder();
  const out = [];
  let at = 0;
  for (let n = 0; n < count && at + 46 <= dir.length; n += 1) {
    if (u32(dir, at) !== 0x02014b50) break;
    const nameLen = u16(dir, at + 28);
    const extraLen = u16(dir, at + 30);
    const commentLen = u16(dir, at + 32);
    out.push({
      name: dec.decode(dir.subarray(at + 46, at + 46 + nameLen)),
      method: u16(dir, at + 10),
      crc: u32(dir, at + 16),
      compressed: u32(dir, at + 20),
      size: u32(dir, at + 24),
      local: u32(dir, at + 42),
    });
    at += 46 + nameLen + extraLen + commentLen;
  }
  return out;
}

/// One entry's bytes, inflated, or null when it is bigger than `max` or
/// cannot be read.
export async function zipEntry(reader, entry, max = READ_MAX) {
  if (entry.size > max) return null;
  const head = await reader.read(entry.local, 30);
  if (u32(head, 0) !== 0x04034b50) return null;
  const start = entry.local + 30 + u16(head, 26) + u16(head, 28);
  const raw = await reader.read(start, entry.compressed);
  if (entry.method === 0) return raw.slice(0, entry.size);
  if (entry.method !== 8) return null;
  return inflateRaw(raw, max);
}

async function inflateRaw(bytes, max) {
  const stream = new Blob([bytes]).stream().pipeThrough(new DecompressionStream("deflate-raw"));
  const reader = stream.getReader();
  const chunks = [];
  let total = 0;
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      total += value.length;
      if (total > max) {
        await reader.cancel().catch(() => {});
        return null;
      }
      chunks.push(value);
    }
  } catch (_) {
    return null;
  }
  const out = new Uint8Array(total);
  let at = 0;
  for (const c of chunks) {
    out.set(c, at);
    at += c.length;
  }
  return out;
}

/// The bytes as text, or null when they are not text (a NUL, or not UTF-8).
export function asText(bytes) {
  if (!bytes) return null;
  const probe = bytes.subarray(0, Math.min(bytes.length, 8192));
  if (probe.includes(0)) return null;
  try {
    return new TextDecoder("utf-8", { fatal: true }).decode(bytes);
  } catch (_) {
    return null;
  }
}

const splitLines = (text) => {
  if (text === "") return [];
  const lines = text.split(/\r?\n/);
  if (lines[lines.length - 1] === "") lines.pop();
  return lines;
};

/// The line edits from `a` to `b`, as `[op, line]` with op " ", "-" or "+".
/// Myers' algorithm on what is left once the common start and end are
/// set aside; past MAX_EDITS edits the middle is shown as replaced whole.
export function diffLines(a, b) {
  let start = 0;
  while (start < a.length && start < b.length && a[start] === b[start]) start += 1;
  let endA = a.length;
  let endB = b.length;
  while (endA > start && endB > start && a[endA - 1] === b[endB - 1]) {
    endA -= 1;
    endB -= 1;
  }
  const A = a.slice(start, endA);
  const B = b.slice(start, endB);
  const middle = myers(A, B) || [...A.map((l) => ["-", l]), ...B.map((l) => ["+", l])];
  return [...a.slice(0, start).map((l) => [" ", l]), ...middle, ...a.slice(endA).map((l) => [" ", l])];
}

function myers(A, B) {
  const N = A.length;
  const M = B.length;
  if (!N) return B.map((l) => ["+", l]);
  if (!M) return A.map((l) => ["-", l]);
  const limit = Math.min(N + M, MAX_EDITS);
  const offset = limit + 1;
  const V = new Int32Array(2 * limit + 3);
  const trace = [];
  let found = -1;
  for (let d = 0; d <= limit && found < 0; d += 1) {
    for (let k = -d; k <= d; k += 2) {
      let x = k === -d || (k !== d && V[offset + k - 1] < V[offset + k + 1]) ? V[offset + k + 1] : V[offset + k - 1] + 1;
      let y = x - k;
      while (x < N && y < M && A[x] === B[y]) {
        x += 1;
        y += 1;
      }
      V[offset + k] = x;
      if (x >= N && y >= M) found = d;
    }
    trace.push(V.slice(offset - d, offset + d + 1));
  }
  if (found < 0) return null;
  const at = (d, k) => trace[d][k + d];
  const ops = [];
  let x = N;
  let y = M;
  for (let d = found; d > 0; d -= 1) {
    const k = x - y;
    const prevK = k === -d || (k !== d && at(d - 1, k - 1) < at(d - 1, k + 1)) ? k + 1 : k - 1;
    const prevX = at(d - 1, prevK);
    const prevY = prevX - prevK;
    while (x > prevX && y > prevY) {
      ops.push([" ", A[x - 1]]);
      x -= 1;
      y -= 1;
    }
    if (x === prevX) {
      ops.push(["+", B[y - 1]]);
      y -= 1;
    } else {
      ops.push(["-", A[x - 1]]);
      x -= 1;
    }
  }
  while (x > 0 && y > 0) {
    ops.push([" ", A[x - 1]]);
    x -= 1;
    y -= 1;
  }
  return ops.reverse();
}

/// Unified hunks from line edits: `[{old_start, old_lines, new_start,
/// new_lines, lines: ["+text", "-text", " text"]}]`, lines numbered from 1.
export function hunksOf(ops, context = CONTEXT) {
  const changed = [];
  ops.forEach((o, i) => {
    if (o[0] !== " ") changed.push(i);
  });
  if (!changed.length) return [];
  // Ranges of ops to show, merged when their context touches.
  const ranges = [];
  for (const i of changed) {
    const from = Math.max(0, i - context);
    const to = Math.min(ops.length - 1, i + context);
    const last = ranges[ranges.length - 1];
    if (last && from <= last[1] + 1) last[1] = Math.max(last[1], to);
    else ranges.push([from, to]);
  }
  // Line numbers at every op.
  const oldAt = [];
  const newAt = [];
  let o = 1;
  let n = 1;
  for (const op of ops) {
    oldAt.push(o);
    newAt.push(n);
    if (op[0] !== "+") o += 1;
    if (op[0] !== "-") n += 1;
  }
  return ranges.map(([from, to]) => {
    const slice = ops.slice(from, to + 1);
    const oldLines = slice.filter((x) => x[0] !== "+").length;
    const newLines = slice.filter((x) => x[0] !== "-").length;
    return {
      old_start: oldLines ? oldAt[from] : oldAt[from] - 1,
      old_lines: oldLines,
      new_start: newLines ? newAt[from] : newAt[from] - 1,
      new_lines: newLines,
      lines: slice.map((x) => x[0] + (x[1].length > LINE_MAX ? `${x[1].slice(0, LINE_MAX)} [line cut at ${LINE_MAX} characters]` : x[1])),
    };
  });
}

/// Hunks cut to `budget` characters: `{hunks, used, truncated}`.
function bounded(hunks, budget) {
  const out = [];
  let used = 0;
  for (const h of hunks) {
    const lines = [];
    for (const line of h.lines) {
      if (used + line.length + 1 > budget) {
        if (lines.length) out.push({ ...h, lines });
        return { hunks: out, used, truncated: true };
      }
      used += line.length + 1;
      lines.push(line);
    }
    out.push(h);
  }
  return { hunks: out, used, truncated: false };
}

// --------------------------------------------------------------- signals

/// Dependencies a Cargo.toml names: Map name -> {kind, spec}.
export function cargoDeps(text) {
  const deps = new Map();
  let kind = null;
  let table = null;
  for (const raw of String(text || "").split(/\r?\n/)) {
    const line = raw.replace(/\s+#.*$/, "").replace(/^#.*$/, "").trim();
    if (!line) continue;
    const head = /^\[([^\]]+)\]$/.exec(line);
    if (head) {
      kind = null;
      table = null;
      const m = /(?:^|\.)((?:dev-|build-)?dependencies)(?:\.("?)([A-Za-z0-9_-]+)\2)?$/.exec(head[1].trim());
      if (m) {
        kind = m[1];
        if (m[3]) {
          table = m[3];
          deps.set(table, { kind, spec: "" });
        }
      }
      continue;
    }
    if (!kind) continue;
    const kv = /^("?)([A-Za-z0-9_.-]+)\1\s*=\s*(.+)$/.exec(line);
    if (!kv) continue;
    const key = kv[2];
    const value = kv[3].trim();
    if (table) {
      const dep = deps.get(table);
      if (key === "version") dep.spec = unquote(value);
      else if (key === "git" && !dep.spec) dep.spec = `git ${unquote(value)}`;
      else if (key === "path" && !dep.spec) dep.spec = "a local path";
      continue;
    }
    if (/^["']/.test(value)) {
      deps.set(key, { kind, spec: unquote(value) });
    } else if (value.startsWith("{")) {
      const version = /\bversion\s*=\s*"([^"]*)"/.exec(value);
      const git = /\bgit\s*=\s*"([^"]*)"/.exec(value);
      const path = /\bpath\s*=/.test(value);
      deps.set(key, { kind, spec: version ? version[1] : git ? `git ${git[1]}` : path ? "a local path" : "" });
    } else if (key.endsWith(".workspace") || /workspace\s*=\s*true/.test(value)) {
      deps.set(key.replace(/\.workspace$/, ""), { kind, spec: "the workspace's" });
    }
  }
  return deps;
}

const unquote = (v) => String(v).replace(/^["']|["']$/g, "");

/// The crates a Cargo.lock resolves from outside the app: Map name -> {versions, source}.
export function cargoLock(text) {
  const out = new Map();
  for (const block of String(text || "").split(/^\[\[package\]\]\s*$/m).slice(1)) {
    const name = /^name\s*=\s*"([^"]+)"/m.exec(block);
    const version = /^version\s*=\s*"([^"]+)"/m.exec(block);
    const source = /^source\s*=\s*"([^"]+)"/m.exec(block);
    if (!name || !version || !source) continue;
    const had = out.get(name[1]) || { versions: [], source: source[1] };
    had.versions.push(version[1]);
    had.versions.sort();
    out.set(name[1], had);
  }
  return out;
}

/// Uses of `unsafe` in Rust source, comments and strings set aside.
export function unsafeCounts(text) {
  const code = String(text || "")
    .replace(/\/\*[\s\S]*?\*\//g, " ")
    .replace(/\/\/[^\n]*/g, " ")
    .replace(/"(?:\\.|[^"\\])*"/g, '""');
  return {
    blocks: (code.match(/\bunsafe\s*\{/g) || []).length,
    items: (code.match(/\bunsafe\s+(?:fn|impl|trait|extern)\b/g) || []).length,
  };
}

const kb = (n) => (n < 1024 ? `${n} bytes` : n < 1024 * 1024 ? `${(n / 1024).toFixed(1)} KB` : `${(n / 1048576).toFixed(1)} MB`);
const shown = (name) => name.slice(SOURCE.length);
const plural = (n, one, many) => `${n} ${n === 1 ? one : many}`;
const isNet = (cap) => /^net\.connect:/.test(cap);

/// The changes between two bundles: `next` (the waiting version) and
/// `prev` (the release it replaces, or null for a new app). `facts` is the
/// store's word on capabilities and provenance:
/// `{added: [{cap, words, asks}], removed: [...], built: "Krate Studio with Claude" | null
/// (null: by hand), ai: bool}`.
export async function codeChanges(next, prev, facts = {}) {
  const nextList = (await zipListing(next)) || [];
  const prevList = prev ? (await zipListing(prev)) || [] : [];
  const sourceOf = (list) => new Map(list.filter((e) => e.name.startsWith(SOURCE) && !e.name.endsWith("/")).map((e) => [e.name, e]));
  const a = sourceOf(prevList);
  const b = sourceOf(nextList);
  const names = [...new Set([...a.keys(), ...b.keys()])].sort((x, y) => {
    // Lock files last: their diff is summarised as dependencies, and the
    // size bound should cut them before the code.
    const lx = /Cargo\.lock$/.test(x) ? 1 : 0;
    const ly = /Cargo\.lock$/.test(y) ? 1 : 0;
    return lx - ly || (x < y ? -1 : x > y ? 1 : 0);
  });
  const budget = { prev: INFLATE_BUDGET, next: INFLATE_BUDGET };
  const read = async (reader, entry, side) => {
    if (!entry || entry.size > READ_MAX || entry.size > budget[side]) return { skipped: true };
    budget[side] -= entry.size;
    const bytes = await zipEntry(reader, entry);
    return bytes ? { bytes } : { skipped: true };
  };

  const files = [];
  let unchanged = 0;
  let used = 0;
  let truncated = false;
  const risks = [];
  const texts = new Map(); // name -> {old, new} text, for the signals
  for (const name of names.slice(0, FILES_MAX)) {
    const was = a.get(name);
    const now = b.get(name);
    if (was && now && was.crc === now.crc && was.size === now.size) {
      unchanged += 1;
      continue;
    }
    const status = !was ? "added" : !now ? "removed" : "changed";
    const file = { path: shown(name), status, size: now ? now.size : 0, old_size: was ? was.size : 0, binary: false, added_lines: 0, removed_lines: 0, hunks: [] };
    const oldRead = was ? await read(prev, was, "prev") : { bytes: new Uint8Array(0) };
    const newRead = now ? await read(next, now, "next") : { bytes: new Uint8Array(0) };
    if (oldRead.skipped || newRead.skipped) {
      file.skipped = "too big to compare here";
      files.push(file);
      continue;
    }
    const oldText = asText(oldRead.bytes);
    const newText = asText(newRead.bytes);
    if (oldText === null || newText === null) {
      file.binary = true;
      if (status === "added") risks.push({ kind: "binary", level: "high", text: `New binary file: ${file.path} (${kb(file.size)})`, file: file.path });
      else if (status === "changed") risks.push({ kind: "binary", level: "note", text: `Binary file changed: ${file.path} (${kb(file.old_size)} to ${kb(file.size)})`, file: file.path });
      files.push(file);
      continue;
    }
    texts.set(name, { old: was ? oldText : null, new: now ? newText : null });
    const ops = diffLines(splitLines(oldText), splitLines(newText));
    file.added_lines = ops.filter((o) => o[0] === "+").length;
    file.removed_lines = ops.filter((o) => o[0] === "-").length;
    // What is left of the total bound, unless too little for a reader to use.
    const room = TOTAL_DIFF_MAX - used < OMIT_UNDER ? 0 : Math.min(FILE_DIFF_MAX, TOTAL_DIFF_MAX - used);
    const cut = room ? bounded(hunksOf(ops), room) : { hunks: [], used: 0, truncated: ops.some((o) => o[0] !== " ") };
    file.hunks = cut.hunks;
    used += cut.used;
    if (cut.truncated) {
      truncated = true;
      if (cut.hunks.length) file.truncated = true;
      else file.omitted = true;
    }
    files.push(file);
  }
  const notCompared = Math.max(0, names.length - FILES_MAX);
  if (notCompared) truncated = true;

  // Permissions and hosts, from the store's comparison.
  const added = Array.isArray(facts.added) ? facts.added : [];
  const removed = Array.isArray(facts.removed) ? facts.removed : [];
  for (const c of added.filter((x) => isNet(x.cap))) {
    risks.push({ kind: "host", level: "high", text: `New network host: ${String(c.cap).slice("net.connect:".length)}` });
  }
  for (const c of added.filter((x) => !isNet(x.cap) && x.asks)) risks.push({ kind: "permission", level: "high", text: `New permission: ${c.words || c.cap}` });
  const quiet = added.filter((x) => !isNet(x.cap) && !x.asks);
  if (prev && quiet.length) risks.push({ kind: "permission", level: "note", text: `Also new, not asked about: ${quiet.map((x) => x.words || x.cap).join(", ")}` });
  if (removed.length) risks.push({ kind: "permission", level: "note", text: `No longer asks to: ${removed.map((x) => x.words || x.cap).join(", ")}` });

  // Dependencies.
  risks.push(...dependencyRows(a, b, texts, Boolean(prev)));

  // Unsafe Rust.
  for (const [name, t] of texts) {
    if (!name.endsWith(".rs") || t.new === null) continue;
    const before = t.old === null ? { blocks: 0, items: 0 } : unsafeCounts(t.old);
    const after = unsafeCounts(t.new);
    const blocks = after.blocks - before.blocks;
    const items = after.items - before.items;
    if (blocks > 0) risks.push({ kind: "unsafe", level: "high", text: `${blocks === 1 ? "1 new unsafe block" : `${blocks} new unsafe blocks`} in ${shown(name)}`, file: shown(name) });
    if (items > 0) risks.push({ kind: "unsafe", level: "high", text: `${items === 1 ? "1 new unsafe function or impl" : `${items} new unsafe functions or impls`} in ${shown(name)}`, file: shown(name) });
  }

  // Build scripts run when the app is built.
  for (const f of files) {
    if (/(^|\/)build\.rs$/.test(f.path) && f.status !== "removed") {
      risks.push({ kind: "build-script", level: "note", text: `${f.status === "added" ? "New build script" : "Build script changed"}: ${f.path} runs when the app is built`, file: f.path });
    }
  }

  // The compiled app.
  const wasm = (list) => list.find((e) => e.name === "code.wasm");
  const nw = wasm(nextList);
  const ow = wasm(prevList);
  const code = { size: nw ? nw.size : null, old_size: ow ? ow.size : null };
  if (nw && ow) {
    const delta = nw.size - ow.size;
    const pct = ow.size ? Math.round((delta / ow.size) * 100) : 0;
    const text = delta === 0
      ? `The compiled app is the same size, ${kb(nw.size)}`
      : `The compiled app ${delta > 0 ? "grew" : "shrank"} from ${kb(ow.size)} to ${kb(nw.size)} (${delta > 0 ? "+" : ""}${pct}%)`;
    risks.push({ kind: "size", level: delta > 0 && pct >= 50 && delta >= 64 * 1024 ? "high" : "note", text });
  } else if (nw) {
    risks.push({ kind: "size", level: "note", text: `The compiled app is ${kb(nw.size)}` });
  }

  // How it was made.
  if ("built" in facts) risks.push({ kind: "made", level: "note", text: !facts.built ? "Made by hand" : facts.ai ? `Made with AI: ${facts.built}` : `Made with ${facts.built}` });

  // Source that is not there cannot be shown: said first.
  if (!b.size) risks.unshift({ kind: "no-source", level: "high", text: "This version carries no source, so its code cannot be shown. Ask its sender to send it again from Krate Studio or with krate publish, which include the source" });
  else if (prev && !a.size) risks.unshift({ kind: "no-source", level: "note", text: "The version it replaces carries no source, so every file shows as new" });

  const changedFiles = files.length;
  return {
    v: CHANGES_VERSION,
    new_app: !prev,
    source: { files: b.size, old_files: prev ? a.size : null },
    files,
    unchanged,
    not_compared: notCompared,
    totals: {
      files: changedFiles,
      added_lines: files.reduce((s, f) => s + f.added_lines, 0),
      removed_lines: files.reduce((s, f) => s + f.removed_lines, 0),
    },
    truncated,
    limits: { total: TOTAL_DIFF_MAX, file: FILE_DIFF_MAX },
    code,
    risks,
  };
}

function dependencyRows(a, b, texts, update) {
  const rows = [];
  const tomlText = (side) => {
    const out = new Map();
    for (const [name, t] of texts) if (/(^|\/)Cargo\.toml$/.test(name) && t[side] !== null) for (const [k, v] of cargoDeps(t[side])) out.set(k, v);
    return out;
  };
  const lockOf = (side, list) => {
    const name = [...list.keys()].find((n) => n === `${SOURCE}Cargo.lock`);
    if (!name) return null;
    const t = texts.get(name);
    if (!t) {
      // An unchanged lock was never read: it is the same on both sides.
      const x = a.get(name);
      const y = b.get(name);
      return x && y && x.crc === y.crc && x.size === y.size ? "same" : null;
    }
    return t[side] === null ? null : cargoLock(t[side]);
  };
  const oldLock = lockOf("old", a);
  const newLock = lockOf("new", b);
  if (!update) {
    const deps = tomlText("new");
    // A new app's Cargo.toml may be unchanged against nothing: read below.
    if (deps.size) {
      const direct = [...deps.entries()].filter(([, v]) => v.kind === "dependencies");
      if (direct.length) rows.push({ kind: "dependency", level: "note", text: `Depends on ${plural(direct.length, "crate", "crates")}: ${direct.map(([k, v]) => (v.spec ? `${k} ${v.spec}` : k)).slice(0, 12).join(", ")}${direct.length > 12 ? ", and more" : ""}` });
    }
    if (newLock && newLock !== "same") {
      rows.push({ kind: "dependency", level: "note", text: `${plural(newLock.size, "crate", "crates")} in all, from Cargo.lock` });
      for (const [name, p] of newLock) if (/^git\+/.test(p.source)) rows.push({ kind: "dependency", level: "high", text: `Dependency from a git repository: ${name} (${p.source.replace(/^git\+/, "").replace(/#.*$/, "")})` });
    }
    return rows;
  }
  const dep = [];
  if (oldLock && newLock && oldLock !== "same" && newLock !== "same") {
    for (const [name, p] of newLock) {
      const had = oldLock.get(name);
      const git = /^git\+/.test(p.source);
      if (!had) dep.push({ kind: "dependency", level: "high", text: `New dependency: ${name} ${p.versions.join(", ")}${git ? `, from a git repository (${p.source.replace(/^git\+/, "").replace(/#.*$/, "")})` : ""}` });
      else if (had.versions.join() !== p.versions.join()) dep.push({ kind: "dependency", level: "note", text: `Dependency changed: ${name} ${had.versions.join(", ")} to ${p.versions.join(", ")}` });
      else if (had.source !== p.source) dep.push({ kind: "dependency", level: "high", text: `Dependency now comes from somewhere else: ${name} (${p.source.replace(/#.*$/, "")})` });
    }
    for (const [name] of oldLock) if (!newLock.has(name)) dep.push({ kind: "dependency", level: "note", text: `Dependency removed: ${name}` });
  } else {
    const before = tomlText("old");
    const after = tomlText("new");
    for (const [name, v] of after) {
      const had = before.get(name);
      const kindWords = v.kind === "dependencies" ? "" : ` (${v.kind.replace(/-dependencies$/, "")} only)`;
      if (!had) dep.push({ kind: "dependency", level: "high", text: `New dependency: ${name}${v.spec ? ` ${v.spec}` : ""}${kindWords}` });
      else if (had.spec !== v.spec) dep.push({ kind: "dependency", level: "note", text: `Dependency changed: ${name} ${had.spec || "any version"} to ${v.spec || "any version"}` });
    }
    for (const [name] of before) if (!after.has(name)) dep.push({ kind: "dependency", level: "note", text: `Dependency removed: ${name}` });
  }
  dep.sort((x, y) => (x.level === y.level ? 0 : x.level === "high" ? -1 : 1));
  rows.push(...dep.slice(0, DEP_ROWS_MAX));
  if (dep.length > DEP_ROWS_MAX) rows.push({ kind: "dependency", level: "note", text: `And ${dep.length - DEP_ROWS_MAX} more dependency changes, in Cargo.lock below` });
  return rows;
}
