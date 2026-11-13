// An app's database in a tab (krate:store/sql, IC-974, K-1149).
//
// sql.wasm is crates/sql-web: the SQLite the desktop links and the rules it
// keeps (crates/appsql), compiled for WebAssembly. It runs here, in the
// app's worker, called synchronously from hosts.js the way the desktop's
// host calls SQLite. The database lives in its memory; hosts.js takes an
// image after every write and the page keeps it in IndexedDB for this app.
//
// SQLite here never touches a file, so the WASI it was compiled against is
// answered with the little it uses (the clock, an empty environment) and
// "not here" for everything else.

const ENOSYS = 52;
const EBADF = 8;

function wasi(memory) {
  const view = () => new DataView(memory().buffer);
  const none = () => ENOSYS;
  return {
    environ_sizes_get(count, size) { view().setUint32(count, 0, true); view().setUint32(size, 0, true); return 0; },
    environ_get: () => 0,
    clock_time_get(_id, _precision, out) {
      view().setBigUint64(out, BigInt(Math.round((performance.timeOrigin + performance.now()) * 1e6)), true);
      return 0;
    },
    // Nothing is printed on purpose; a write is accepted and dropped.
    fd_write(_fd, iovs, count, written) {
      let n = 0;
      for (let i = 0; i < count; i++) n += view().getUint32(iovs + i * 8 + 4, true);
      view().setUint32(written, n, true);
      return 0;
    },
    fd_prestat_get: () => EBADF,
    proc_exit(code) { throw new Error(`the database stopped (${code})`); },
    fd_close: none, fd_fdstat_get: none, fd_fdstat_set_flags: none, fd_filestat_get: none,
    fd_filestat_set_size: none, fd_prestat_dir_name: none, fd_read: none, fd_seek: none, fd_sync: none,
    path_create_directory: none, path_filestat_get: none, path_filestat_set_times: none, path_open: none,
    path_readlink: none, path_remove_directory: none, path_unlink_file: none, poll_oneoff: none,
    random_get: none, sched_yield: () => 0, args_sizes_get: none, args_get: none,
  };
}

// Compiled while it downloads when the response says it is WebAssembly (the
// hub serves it as application/wasm); from the whole file otherwise, since a
// streaming compile refuses any other type and has used the body by then.
// `fetched` is the download already under way, when the caller started it
// early; otherwise it starts here.
async function instantiate(url, imports, fetched) {
  const response = await (fetched || fetch(url));
  if (!response.ok) throw new Error(`sql.wasm answered ${response.status}`);
  const type = response.headers.get('content-type') || '';
  if (WebAssembly.instantiateStreaming && /^application\/wasm\b/i.test(type)) {
    return WebAssembly.instantiateStreaming(response, imports);
  }
  return WebAssembly.instantiate(await response.arrayBuffer(), imports);
}

// `image` is what the browser kept for this app (null for none yet).
export async function openSqlDatabase({ url, granted, image, fetched }) {
  let exports;
  const { instance } = await instantiate(url, { wasi_snapshot_preview1: wasi(() => exports.memory) }, fetched);
  exports = instance.exports;
  const enc = new TextEncoder();
  const dec = new TextDecoder();
  const out = () => new Uint8Array(exports.memory.buffer, exports.krate_sql_out_ptr(), exports.krate_sql_out_len()).slice();
  const withBytes = (data, fn) => {
    const len = data.length;
    const ptr = len ? exports.krate_sql_alloc(len) : 0;
    if (len) new Uint8Array(exports.memory.buffer, ptr, len).set(data);
    try { return fn(ptr, len); } finally { if (len) exports.krate_sql_free(ptr, len); }
  };
  const kept = image ? new Uint8Array(image) : new Uint8Array(0);
  // A saved database SQLite cannot read is reported on every call, never
  // replaced: what the person kept stays kept.
  const failed = withBytes(kept, (ptr, len) => exports.krate_sql_open(granted ? 1 : 0, ptr, len));
  const broken = failed ? dec.decode(out()) : null;
  return {
    broken,
    call(request) {
      withBytes(enc.encode(JSON.stringify(request)), (ptr, len) => exports.krate_sql_call(ptr, len));
      return JSON.parse(dec.decode(out()));
    },
    dirty: () => exports.krate_sql_dirty() === 1,
    image() {
      if (exports.krate_sql_image() !== 0) throw new Error(dec.decode(out()));
      return out();
    },
  };
}
