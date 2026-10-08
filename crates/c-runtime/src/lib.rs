//! C and C++ programs as Krate apps.
//!
//! A program built with wasi-sdk calls the WASI preview-1 functions
//! (`fd_write`, `path_open`, `clock_time_get`, ...) through wasi-libc. A Krate
//! app may import Krate's interfaces and nothing else -- an app importing
//! WASI is refused -- so those calls are answered here, inside the app:
//! wasi-libc reaches each one through a symbol named
//! `__imported_wasi_snapshot_preview1_<name>`, and defining that symbol in
//! the app resolves the call before it could ever become an import.
//!
//! What each call becomes:
//! - files: `krate:fs` paths, sandbox-relative like every Krate app's, with
//!   folders the person picked under `picked/<token>/`. One preopened
//!   directory, `/`, is the app's root.
//! - stdout and stderr: `krate:io/stdio`. stdin reads nothing.
//! - clocks, sleep: `krate:time`. Random: `krate:random`.
//! - args: whatever the app sets with [`set_args`]. No environment.
//! - sockets: not supported (Krate's network is `krate:net`).
//! - dlopen: an app is one module, so there is nothing to open; `dlopen`
//!   fails and `dlerror` says why, and a program takes its own error path.
//!
//! Use: link wasi-sdk's `libc.a` (and `libc++.a`, `libc++abi.a` for C++)
//! into the app, call [`start`] once, then the program's entry point.

#![no_std]
#![allow(
    clippy::missing_safety_doc,
    clippy::too_many_arguments,
    static_mut_refs
)]
extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use krate::bindings::krate::fs::files::{self, File, OpenMode};
use krate::bindings::krate::fs::types::FsError;
use krate::bindings::krate::io::stdio;
use krate::bindings::krate::time::{clock, sleep};

// ---- WASI numbers ---------------------------------------------------------
const SUCCESS: i32 = 0;
const EACCES: i32 = 2;
const EBADF: i32 = 8;
const EEXIST: i32 = 20;
const EINVAL: i32 = 28;
const EIO: i32 = 29;
const EISDIR: i32 = 31;
const ENOENT: i32 = 44;
const ENOTDIR: i32 = 54;
const ENOTSUP: i32 = 58;

const FT_CHAR: u8 = 2;
const FT_DIR: u8 = 3;
const FT_FILE: u8 = 4;

const O_CREAT: i32 = 1;
const O_DIRECTORY: i32 = 2;
const O_EXCL: i32 = 4;
const O_TRUNC: i32 = 8;
const FD_APPEND: i32 = 1;
const RIGHT_READ: i64 = 1 << 1;
const RIGHT_WRITE: i64 = 1 << 6;

enum Entry {
    Stdin,
    Out(bool),
    Dir(String),
    File { file: File, pos: u64, path: String },
}

static mut FDS: Vec<Option<Entry>> = Vec::new();
static mut ARGS: Vec<String> = Vec::new();
static mut STARTED: bool = false;
static mut EXIT_CODE: Option<i32> = None;

extern "C" {
    fn fflush(stream: *mut core::ffi::c_void) -> i32;
}

/// Flush every C stream. Call when the program's entry point returns:
/// libc flushes at `exit`, and a program whose `main` simply returns to the
/// app never reaches it.
pub fn finish() {
    unsafe {
        fflush(core::ptr::null_mut());
    }
}

/// Set the program's `argv` (the first is its name). Call before [`start`].
pub fn set_args(args: &[&str]) {
    unsafe {
        ARGS = args.iter().map(|a| a.to_string()).collect();
    }
}

/// Make the C runtime ready: standard streams, the preopened root, and the
/// program's static constructors (C++ `iostream`, globals). Once only.
pub fn start() {
    unsafe {
        if STARTED {
            return;
        }
        STARTED = true;
        FDS = Vec::new();
        FDS.push(Some(Entry::Stdin));
        FDS.push(Some(Entry::Out(false)));
        FDS.push(Some(Entry::Out(true)));
        FDS.push(Some(Entry::Dir(String::new())));
    }
    // Through the SDK's own guard, never directly: the app's exported entry
    // has already run the constructors through it, and running them a second
    // time made every self-registering C++ list loop forever (K-1009).
    wit_bindgen_rt::run_ctors_once();
}

/// The code a program passed to `exit`, when it has called it.
pub fn exit_code() -> Option<i32> {
    unsafe { EXIT_CODE }
}

fn fds() -> &'static mut Vec<Option<Entry>> {
    unsafe { &mut FDS }
}

fn put_fd(entry: Entry) -> i32 {
    let fds = fds();
    if let Some(i) = fds.iter().position(|e| e.is_none()) {
        fds[i] = Some(entry);
        return i as i32;
    }
    fds.push(Some(entry));
    (fds.len() - 1) as i32
}

fn entry(fd: i32) -> Option<&'static mut Entry> {
    fds().get_mut(fd as usize).and_then(|e| e.as_mut())
}

unsafe fn bytes<'a>(ptr: i32, len: i32) -> &'a [u8] {
    core::slice::from_raw_parts(ptr as usize as *const u8, len as usize)
}

unsafe fn bytes_mut<'a>(ptr: i32, len: i32) -> &'a mut [u8] {
    core::slice::from_raw_parts_mut(ptr as usize as *mut u8, len as usize)
}

unsafe fn put_u64(ptr: i32, v: u64) {
    (ptr as usize as *mut u64).write_unaligned(v);
}

unsafe fn put_u32(ptr: i32, v: u32) {
    (ptr as usize as *mut u32).write_unaligned(v);
}

unsafe fn get_u32(ptr: i32) -> u32 {
    (ptr as usize as *const u32).read_unaligned()
}

fn errno(err: &FsError) -> i32 {
    match err {
        FsError::NotFound => ENOENT,
        FsError::PermissionDenied => EACCES,
        FsError::AlreadyExists => EEXIST,
        FsError::InvalidPath => ENOENT,
        FsError::NotADirectory => ENOTDIR,
        FsError::IsADirectory => EISDIR,
        FsError::Io(_) => EIO,
    }
}

/// A WASI path under directory `fd`, as a Krate path: sandbox-relative, no
/// `.` segments, `..` folded where it stays inside.
unsafe fn resolve(fd: i32, ptr: i32, len: i32) -> Result<String, i32> {
    let Some(Entry::Dir(base)) = entry(fd) else {
        return Err(EBADF);
    };
    let rel = core::str::from_utf8(bytes(ptr, len)).map_err(|_| EINVAL)?;
    let mut parts: Vec<&str> = base.split('/').filter(|p| !p.is_empty()).collect();
    for seg in rel.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return Err(EACCES);
                }
            }
            s => parts.push(s),
        }
    }
    Ok(parts.join("/"))
}

/// Krate names the root `.`; WASI's root directory is the empty path here.
fn krate_path(path: &str) -> &str {
    if path.is_empty() {
        "."
    } else {
        path
    }
}

/// Is `path` a directory (Some(true)), a file (Some(false)), or absent?
///
/// A grant like `fs.read:./saves/**` covers what is inside `saves` and not
/// the folder's own entry, so `stat` on the folder can be refused while
/// listing it is allowed. A folder that lists is a folder.
fn kind_of(path: &str) -> Option<bool> {
    match files::stat(path) {
        Ok(st) => Some(st.is_dir),
        Err(FsError::NotFound) => None,
        Err(_) => files::list(path).ok().map(|_| true),
    }
}

unsafe fn write_filestat(ptr: i32, filetype: u8, size: u64, mtime_ms: u64) {
    core::ptr::write_bytes(ptr as usize as *mut u8, 0, 64);
    *((ptr + 16) as usize as *mut u8) = filetype;
    put_u64(ptr + 24, 1);
    put_u64(ptr + 32, size);
    let ns = mtime_ms.saturating_mul(1_000_000);
    put_u64(ptr + 40, ns);
    put_u64(ptr + 48, ns);
    put_u64(ptr + 56, ns);
}

// ---- the WASI calls ---------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn __imported_wasi_snapshot_preview1_args_sizes_get(
    argc: i32,
    size: i32,
) -> i32 {
    let args = &ARGS;
    put_u32(argc, args.len() as u32);
    put_u32(size, args.iter().map(|a| a.len() as u32 + 1).sum());
    SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn __imported_wasi_snapshot_preview1_args_get(argv: i32, buf: i32) -> i32 {
    let mut at = buf;
    for (i, a) in ARGS.iter().enumerate() {
        put_u32(argv + 4 * i as i32, at as u32);
        let out = bytes_mut(at, a.len() as i32 + 1);
        out[..a.len()].copy_from_slice(a.as_bytes());
        out[a.len()] = 0;
        at += a.len() as i32 + 1;
    }
    SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn __imported_wasi_snapshot_preview1_environ_sizes_get(
    n: i32,
    size: i32,
) -> i32 {
    put_u32(n, 0);
    put_u32(size, 0);
    SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn __imported_wasi_snapshot_preview1_environ_get(_e: i32, _b: i32) -> i32 {
    SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn __imported_wasi_snapshot_preview1_clock_res_get(
    _id: i32,
    out: i32,
) -> i32 {
    put_u64(out, 1_000);
    SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn __imported_wasi_snapshot_preview1_clock_time_get(
    id: i32,
    _p: i64,
    out: i32,
) -> i32 {
    let ns = if id == 0 {
        clock::now_millis().saturating_mul(1_000_000)
    } else {
        clock::monotonic_nanos()
    };
    put_u64(out, ns);
    SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn __imported_wasi_snapshot_preview1_random_get(buf: i32, len: i32) -> i32 {
    let out = bytes_mut(buf, len);
    let mut filled = 0;
    while filled < out.len() {
        let want = (out.len() - filled).min(4096) as u32;
        match krate::random::bytes(want) {
            Ok(b) => {
                out[filled..filled + b.len()].copy_from_slice(&b);
                filled += b.len();
            }
            Err(_) => return EIO,
        }
    }
    SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn __imported_wasi_snapshot_preview1_proc_exit(code: i32) -> ! {
    EXIT_CODE = Some(code);
    core::arch::wasm32::unreachable()
}

#[no_mangle]
pub unsafe extern "C" fn __imported_wasi_snapshot_preview1_sched_yield() -> i32 {
    SUCCESS
}

/// Clock subscriptions sleep; anything else returns at once.
#[no_mangle]
pub unsafe extern "C" fn __imported_wasi_snapshot_preview1_poll_oneoff(
    inp: i32,
    out: i32,
    n: i32,
    nevents: i32,
) -> i32 {
    let mut longest_ns: u64 = 0;
    for i in 0..n {
        let sub = inp + i * 48;
        if *((sub + 8) as usize as *const u8) == 0 {
            let timeout = ((sub + 24) as usize as *const u64).read_unaligned();
            longest_ns = longest_ns.max(timeout);
        }
        let ev = out + i * 32;
        core::ptr::write_bytes(ev as usize as *mut u8, 0, 32);
        put_u64(ev, ((sub) as usize as *const u64).read_unaligned());
        *((ev + 10) as usize as *mut u8) = *((sub + 8) as usize as *const u8);
    }
    if longest_ns > 0 {
        sleep::sleep_millis((longest_ns / 1_000_000).max(1) as u32);
    }
    put_u32(nevents, n as u32);
    SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn __imported_wasi_snapshot_preview1_fd_prestat_get(
    fd: i32,
    out: i32,
) -> i32 {
    if fd == 3 {
        *(out as usize as *mut u8) = 0;
        put_u32(out + 4, 1);
        return SUCCESS;
    }
    EBADF
}

#[no_mangle]
pub unsafe extern "C" fn __imported_wasi_snapshot_preview1_fd_prestat_dir_name(
    fd: i32,
    path: i32,
    len: i32,
) -> i32 {
    if fd == 3 && len >= 1 {
        *(path as usize as *mut u8) = b'/';
        return SUCCESS;
    }
    EBADF
}

#[no_mangle]
pub unsafe extern "C" fn __imported_wasi_snapshot_preview1_fd_fdstat_get(fd: i32, out: i32) -> i32 {
    let Some(e) = entry(fd) else { return EBADF };
    core::ptr::write_bytes(out as usize as *mut u8, 0, 24);
    let ft = match e {
        Entry::Stdin | Entry::Out(_) => FT_CHAR,
        Entry::Dir(_) => FT_DIR,
        Entry::File { .. } => FT_FILE,
    };
    *(out as usize as *mut u8) = ft;
    // A stream with no seek or tell rights is what libc takes for a
    // terminal, so stdout is line-buffered: a program's printf shows up as it
    // prints, not only when a buffer fills.
    let rights = if ft == FT_CHAR {
        u64::MAX & !((1 << 2) | (1 << 5))
    } else {
        u64::MAX
    };
    put_u64(out + 8, rights);
    put_u64(out + 16, rights);
    SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn __imported_wasi_snapshot_preview1_fd_fdstat_set_flags(
    _fd: i32,
    _f: i32,
) -> i32 {
    SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn __imported_wasi_snapshot_preview1_fd_fdstat_set_rights(
    _fd: i32,
    _b: i64,
    _i: i64,
) -> i32 {
    SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn __imported_wasi_snapshot_preview1_fd_close(fd: i32) -> i32 {
    match fds().get_mut(fd as usize) {
        Some(slot @ Some(_)) if fd > 2 => {
            *slot = None;
            SUCCESS
        }
        Some(Some(_)) => SUCCESS,
        _ => EBADF,
    }
}

#[no_mangle]
pub unsafe extern "C" fn __imported_wasi_snapshot_preview1_fd_write(
    fd: i32,
    iovs: i32,
    n: i32,
    written: i32,
) -> i32 {
    let Some(e) = entry(fd) else { return EBADF };
    let mut total = 0u32;
    for i in 0..n {
        let buf = get_u32(iovs + i * 8) as i32;
        let len = get_u32(iovs + i * 8 + 4) as i32;
        if len == 0 {
            continue;
        }
        let data = bytes(buf, len);
        match e {
            Entry::Out(_) => {
                let _ = stdio::stdout().write(data);
                total += len as u32;
            }
            Entry::File { file, pos, .. } => match file.write(data) {
                Ok(w) => {
                    *pos += w as u64;
                    total += w;
                }
                Err(err) => return errno(&err),
            },
            _ => return EBADF,
        }
    }
    put_u32(written, total);
    SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn __imported_wasi_snapshot_preview1_fd_read(
    fd: i32,
    iovs: i32,
    n: i32,
    read: i32,
) -> i32 {
    let Some(e) = entry(fd) else { return EBADF };
    let mut total = 0u32;
    if let Entry::File { file, pos, .. } = e {
        for i in 0..n {
            let buf = get_u32(iovs + i * 8) as i32;
            let len = get_u32(iovs + i * 8 + 4);
            if len == 0 {
                continue;
            }
            match file.read(len) {
                Ok(data) => {
                    bytes_mut(buf, data.len() as i32).copy_from_slice(&data);
                    *pos += data.len() as u64;
                    total += data.len() as u32;
                    if (data.len() as u32) < len {
                        break;
                    }
                }
                Err(err) => return errno(&err),
            }
        }
    } else if !matches!(e, Entry::Stdin) {
        return EBADF;
    }
    put_u32(read, total);
    SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn __imported_wasi_snapshot_preview1_fd_seek(
    fd: i32,
    offset: i64,
    whence: i32,
    out: i32,
) -> i32 {
    let Some(Entry::File { file, pos, .. }) = entry(fd) else {
        return EBADF;
    };
    let target = match whence {
        0 => offset,
        1 => *pos as i64 + offset,
        2 => match file.stat() {
            Ok(st) => st.size as i64 + offset,
            Err(err) => return errno(&err),
        },
        _ => return EINVAL,
    };
    if target < 0 {
        return EINVAL;
    }
    match file.seek_set(target as u64) {
        Ok(p) => {
            *pos = p;
            put_u64(out, p);
            SUCCESS
        }
        Err(err) => errno(&err),
    }
}

#[no_mangle]
pub unsafe extern "C" fn __imported_wasi_snapshot_preview1_fd_tell(fd: i32, out: i32) -> i32 {
    let Some(Entry::File { pos, .. }) = entry(fd) else {
        return EBADF;
    };
    put_u64(out, *pos);
    SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn __imported_wasi_snapshot_preview1_fd_pread(
    fd: i32,
    iovs: i32,
    n: i32,
    offset: i64,
    read: i32,
) -> i32 {
    let Some(Entry::File { file, pos, .. }) = entry(fd) else {
        return EBADF;
    };
    let back = *pos;
    if let Err(err) = file.seek_set(offset as u64) {
        return errno(&err);
    }
    *pos = offset as u64;
    let r = __imported_wasi_snapshot_preview1_fd_read(fd, iovs, n, read);
    if let Some(Entry::File { file, pos, .. }) = entry(fd) {
        let _ = file.seek_set(back);
        *pos = back;
    }
    r
}

#[no_mangle]
pub unsafe extern "C" fn __imported_wasi_snapshot_preview1_fd_pwrite(
    fd: i32,
    iovs: i32,
    n: i32,
    offset: i64,
    written: i32,
) -> i32 {
    let Some(Entry::File { file, pos, .. }) = entry(fd) else {
        return EBADF;
    };
    let back = *pos;
    if let Err(err) = file.seek_set(offset as u64) {
        return errno(&err);
    }
    *pos = offset as u64;
    let r = __imported_wasi_snapshot_preview1_fd_write(fd, iovs, n, written);
    if let Some(Entry::File { file, pos, .. }) = entry(fd) {
        let _ = file.seek_set(back);
        *pos = back;
    }
    r
}

#[no_mangle]
pub unsafe extern "C" fn __imported_wasi_snapshot_preview1_fd_filestat_get(
    fd: i32,
    out: i32,
) -> i32 {
    match entry(fd) {
        Some(Entry::File { file, .. }) => match file.stat() {
            Ok(st) => {
                write_filestat(out, FT_FILE, st.size, st.modified_millis);
                SUCCESS
            }
            Err(err) => errno(&err),
        },
        Some(Entry::Dir(_)) => {
            write_filestat(out, FT_DIR, 0, 0);
            SUCCESS
        }
        Some(_) => {
            write_filestat(out, FT_CHAR, 0, 0);
            SUCCESS
        }
        None => EBADF,
    }
}

#[no_mangle]
pub unsafe extern "C" fn __imported_wasi_snapshot_preview1_path_filestat_get(
    fd: i32,
    _flags: i32,
    ptr: i32,
    len: i32,
    out: i32,
) -> i32 {
    let path = match resolve(fd, ptr, len) {
        Ok(p) => p,
        Err(e) => return e,
    };
    if path.is_empty() {
        write_filestat(out, FT_DIR, 0, 0);
        return SUCCESS;
    }
    match files::stat(&path) {
        Ok(st) => {
            write_filestat(
                out,
                if st.is_dir { FT_DIR } else { FT_FILE },
                st.size,
                st.modified_millis,
            );
            SUCCESS
        }
        Err(FsError::NotFound) => ENOENT,
        Err(err) => {
            if kind_of(&path) == Some(true) {
                write_filestat(out, FT_DIR, 0, 0);
                SUCCESS
            } else {
                errno(&err)
            }
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn __imported_wasi_snapshot_preview1_path_open(
    fd: i32,
    _dirflags: i32,
    ptr: i32,
    len: i32,
    oflags: i32,
    rights: i64,
    _inheriting: i64,
    fdflags: i32,
    out: i32,
) -> i32 {
    let path = match resolve(fd, ptr, len) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let existing = if path.is_empty() {
        Some(true)
    } else {
        kind_of(&path)
    };
    if oflags & O_EXCL != 0 && existing.is_some() {
        return EEXIST;
    }
    if existing == Some(true) || oflags & O_DIRECTORY != 0 {
        if existing != Some(true) {
            return if existing.is_none() { ENOENT } else { ENOTDIR };
        }
        put_u32(out, put_fd(Entry::Dir(path)) as u32);
        return SUCCESS;
    }
    let wants_write = rights & RIGHT_WRITE != 0 || oflags & (O_CREAT | O_TRUNC) != 0;
    let mode = if fdflags & FD_APPEND != 0 {
        OpenMode::Append
    } else if oflags & O_TRUNC != 0 || (oflags & O_CREAT != 0 && existing.is_none()) {
        if rights & RIGHT_READ != 0 {
            // "w+": emptied, then read AND written. Krate's write mode cannot
            // be read from, and its read-write mode does not empty the file,
            // so empty it with one, then open it with the other. Mapping this
            // to write-only made every read come back empty -- a demo the
            // engine built by writing a file and reading it back was all
            // zeros (K-1012).
            match files::open(&path, OpenMode::Write) {
                Ok(emptied) => drop(emptied),
                Err(err) => return errno(&err),
            }
            OpenMode::ReadWrite
        } else {
            OpenMode::Write
        }
    } else if wants_write {
        OpenMode::ReadWrite
    } else {
        OpenMode::Read
    };
    match files::open(&path, mode) {
        Ok(file) => {
            put_u32(out, put_fd(Entry::File { file, pos: 0, path }) as u32);
            SUCCESS
        }
        Err(err) => errno(&err),
    }
}

/// Directory entries as WASI dirents: 24-byte header, then the name.
#[no_mangle]
pub unsafe extern "C" fn __imported_wasi_snapshot_preview1_fd_readdir(
    fd: i32,
    buf: i32,
    len: i32,
    cookie: i64,
    used: i32,
) -> i32 {
    let Some(Entry::Dir(path)) = entry(fd) else {
        return EBADF;
    };
    let names = match files::list(krate_path(path)) {
        Ok(n) => n,
        Err(err) => return errno(&err),
    };
    let base = path.clone();
    let out = bytes_mut(buf, len);
    let mut at = 0usize;
    for (i, name) in names.iter().enumerate().skip(cookie as usize) {
        let full = if base.is_empty() {
            name.clone()
        } else {
            alloc::format!("{base}/{name}")
        };
        let ft = match files::stat(&full) {
            Ok(st) if st.is_dir => FT_DIR,
            _ => FT_FILE,
        };
        let mut head = [0u8; 24];
        head[0..8].copy_from_slice(&((i + 1) as u64).to_le_bytes());
        head[8..16].copy_from_slice(&((i + 1) as u64).to_le_bytes());
        head[16..20].copy_from_slice(&(name.len() as u32).to_le_bytes());
        head[20] = ft;
        for b in head.iter().chain(name.as_bytes()) {
            if at >= out.len() {
                put_u32(used, at as u32);
                return SUCCESS;
            }
            out[at] = *b;
            at += 1;
        }
    }
    put_u32(used, at as u32);
    SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn __imported_wasi_snapshot_preview1_path_create_directory(
    fd: i32,
    ptr: i32,
    len: i32,
) -> i32 {
    match resolve(fd, ptr, len) {
        Ok(p) => match files::mkdir(&p) {
            Ok(()) => SUCCESS,
            Err(err) => errno(&err),
        },
        Err(e) => e,
    }
}

#[no_mangle]
pub unsafe extern "C" fn __imported_wasi_snapshot_preview1_path_remove_directory(
    fd: i32,
    ptr: i32,
    len: i32,
) -> i32 {
    match resolve(fd, ptr, len) {
        Ok(p) => match files::remove_dir(&p) {
            Ok(()) => SUCCESS,
            Err(err) => errno(&err),
        },
        Err(e) => e,
    }
}

#[no_mangle]
pub unsafe extern "C" fn __imported_wasi_snapshot_preview1_path_unlink_file(
    fd: i32,
    ptr: i32,
    len: i32,
) -> i32 {
    match resolve(fd, ptr, len) {
        Ok(p) => match files::remove_file(&p) {
            Ok(()) => SUCCESS,
            Err(err) => errno(&err),
        },
        Err(e) => e,
    }
}

#[no_mangle]
pub unsafe extern "C" fn __imported_wasi_snapshot_preview1_path_rename(
    fd: i32,
    a: i32,
    al: i32,
    nfd: i32,
    b: i32,
    bl: i32,
) -> i32 {
    let from = match resolve(fd, a, al) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let to = match resolve(nfd, b, bl) {
        Ok(p) => p,
        Err(e) => return e,
    };
    match files::rename(&from, &to) {
        Ok(()) => SUCCESS,
        Err(err) => errno(&err),
    }
}

#[no_mangle]
pub unsafe extern "C" fn __imported_wasi_snapshot_preview1_fd_renumber(fd: i32, to: i32) -> i32 {
    let fds = fds();
    let Some(e) = fds.get_mut(fd as usize).and_then(|e| e.take()) else {
        return EBADF;
    };
    while fds.len() <= to as usize {
        fds.push(None);
    }
    fds[to as usize] = Some(e);
    SUCCESS
}

// Calls a sandboxed app answers with "fine" or "not here".
#[no_mangle]
pub unsafe extern "C" fn __imported_wasi_snapshot_preview1_fd_sync(_fd: i32) -> i32 {
    SUCCESS
}
#[no_mangle]
pub unsafe extern "C" fn __imported_wasi_snapshot_preview1_fd_datasync(_fd: i32) -> i32 {
    SUCCESS
}
#[no_mangle]
pub unsafe extern "C" fn __imported_wasi_snapshot_preview1_fd_advise(
    _fd: i32,
    _o: i64,
    _l: i64,
    _a: i32,
) -> i32 {
    SUCCESS
}
#[no_mangle]
pub unsafe extern "C" fn __imported_wasi_snapshot_preview1_fd_allocate(
    _fd: i32,
    _o: i64,
    _l: i64,
) -> i32 {
    SUCCESS
}
#[no_mangle]
pub unsafe extern "C" fn __imported_wasi_snapshot_preview1_fd_filestat_set_size(
    _fd: i32,
    _s: i64,
) -> i32 {
    ENOTSUP
}
#[no_mangle]
pub unsafe extern "C" fn __imported_wasi_snapshot_preview1_fd_filestat_set_times(
    _fd: i32,
    _a: i64,
    _m: i64,
    _f: i32,
) -> i32 {
    SUCCESS
}
#[no_mangle]
pub unsafe extern "C" fn __imported_wasi_snapshot_preview1_path_filestat_set_times(
    _fd: i32,
    _f: i32,
    _p: i32,
    _l: i32,
    _a: i64,
    _m: i64,
    _ff: i32,
) -> i32 {
    SUCCESS
}
#[no_mangle]
pub unsafe extern "C" fn __imported_wasi_snapshot_preview1_path_link(
    _a: i32,
    _b: i32,
    _c: i32,
    _d: i32,
    _e: i32,
    _f: i32,
    _g: i32,
) -> i32 {
    ENOTSUP
}
#[no_mangle]
pub unsafe extern "C" fn __imported_wasi_snapshot_preview1_path_readlink(
    _fd: i32,
    _p: i32,
    _l: i32,
    _b: i32,
    _bl: i32,
    _o: i32,
) -> i32 {
    EINVAL
}
#[no_mangle]
pub unsafe extern "C" fn __imported_wasi_snapshot_preview1_path_symlink(
    _a: i32,
    _b: i32,
    _c: i32,
    _d: i32,
    _e: i32,
) -> i32 {
    ENOTSUP
}
#[no_mangle]
pub unsafe extern "C" fn __imported_wasi_snapshot_preview1_sock_accept(
    _fd: i32,
    _f: i32,
    _o: i32,
) -> i32 {
    ENOTSUP
}
#[no_mangle]
pub unsafe extern "C" fn __imported_wasi_snapshot_preview1_sock_recv(
    _fd: i32,
    _a: i32,
    _b: i32,
    _c: i32,
    _d: i32,
    _e: i32,
) -> i32 {
    ENOTSUP
}
#[no_mangle]
pub unsafe extern "C" fn __imported_wasi_snapshot_preview1_sock_send(
    _fd: i32,
    _a: i32,
    _b: i32,
    _c: i32,
    _d: i32,
) -> i32 {
    ENOTSUP
}
#[no_mangle]
pub unsafe extern "C" fn __imported_wasi_snapshot_preview1_sock_shutdown(_fd: i32, _h: i32) -> i32 {
    ENOTSUP
}

// ---- dynamic libraries ----------------------------------------------------
// wasi-libc declares these and defines none of them for a static program. A
// program that can live without a plugin (a game's optional bot or metamod
// library) asks, gets no, and carries on.

static DL_ERROR: &[u8] = b"no dynamic libraries in a Krate app\0";

#[no_mangle]
pub extern "C" fn dlopen(_file: *const core::ffi::c_char, _mode: i32) -> *mut core::ffi::c_void {
    core::ptr::null_mut()
}

#[no_mangle]
pub extern "C" fn dlsym(
    _handle: *mut core::ffi::c_void,
    _name: *const core::ffi::c_char,
) -> *mut core::ffi::c_void {
    core::ptr::null_mut()
}

#[no_mangle]
pub extern "C" fn dlclose(_handle: *mut core::ffi::c_void) -> i32 {
    0
}

#[no_mangle]
pub extern "C" fn dlerror() -> *const core::ffi::c_char {
    DL_ERROR.as_ptr() as *const core::ffi::c_char
}
