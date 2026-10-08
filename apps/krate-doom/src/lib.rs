//! Doom, as a Krate app.
//!
//! The game is id Software's 1993 Doom through doomgeneric (c/doom, GPL-2.0),
//! compiled from C to WebAssembly by build.rs with a small C library of our
//! own (c/libc.c). This file is the platform underneath it: the six
//! functions doomgeneric asks every port for (draw a frame, the clock, sleep,
//! the next key, a title, init) and the four the C library hands over
//! (memory, text out, the WAD, leaving). Everything goes through Krate's own
//! interfaces -- one canvas, the clock, held keys -- so the app imports no
//! WASI and needs only a window.
//!
//! The WAD is the shareware episode, built into the app.

#![no_std]
#![allow(static_mut_refs, clippy::missing_safety_doc)]
extern crate alloc;

use alloc::alloc::{alloc, dealloc, realloc, Layout};
use alloc::vec::Vec;
use core::ffi::{c_char, c_int, c_uchar, c_void};
use krate::bindings::krate::io::{args, stdio};
use krate::bindings::krate::time::{clock, sleep};
use krate::gfx::{canvas2d, types as gfx};
use krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;
const RES_X: usize = 320;
const RES_Y: usize = 200;

/// The shareware WAD, built in. It lives beside the app (../krate-doom-wad)
/// rather than in it, so it is in the app once -- inside the code -- and not
/// packed a second time as source; build.rs says where to get it.
static WAD: &[u8] = include_bytes!(env!("DOOM_WAD"));

extern "C" {
    static DG_ScreenBuffer: *mut u32;
    fn doomgeneric_Create(argc: c_int, argv: *mut *mut c_char);
    fn doomgeneric_Tick();
}

struct Host {
    win: u64,
    canvas: u64,
    rgba: Vec<u8>,
    frames: u64,
    held: [bool; KEYS.len()],
    queue: Vec<(bool, u8)>,
}

static mut HOST: Option<Host> = None;

fn host() -> &'static mut Host {
    unsafe { HOST.as_mut().expect("host") }
}

/// Krate's key names and the Doom key each one presses. Krate reports no
/// Ctrl or Shift as keys, so firing is Space and opening doors is E, with
/// WASD beside the arrows the way a player today expects.
const KEYS: [(&str, u8); 24] = [
    ("ArrowUp", 0xad),
    ("ArrowDown", 0xaf),
    ("ArrowLeft", 0xac),
    ("ArrowRight", 0xae),
    ("w", 0xad),
    ("s", 0xaf),
    ("a", 0xa0),
    ("d", 0xa1),
    ("Space", 0xa3),
    ("e", 0xa2),
    ("Enter", 13),
    ("Escape", 27),
    ("Tab", 9),
    ("y", b'y'),
    ("n", b'n'),
    ("1", b'1'),
    ("2", b'2'),
    ("3", b'3'),
    ("4", b'4'),
    ("5", b'5'),
    ("6", b'6'),
    ("7", b'7'),
    ("Backspace", 0x7f),
    ("m", b'm'),
];

/// Turn this frame's held keys into the presses and releases Doom reads.
fn poll_keys() {
    let h = host();
    for (i, (name, code)) in KEYS.iter().enumerate() {
        let now = events::key_held(name);
        if now != h.held[i] {
            h.held[i] = now;
            h.queue.push((now, *code));
        }
    }
}

// ---- what doomgeneric asks of a platform ---------------------------------

#[no_mangle]
pub extern "C" fn DG_Init() {}

#[no_mangle]
pub extern "C" fn DG_DrawFrame() {
    let h = host();
    let src = unsafe { core::slice::from_raw_parts(DG_ScreenBuffer, RES_X * RES_Y) };
    for (px, out) in src.iter().zip(h.rgba.chunks_exact_mut(4)) {
        out[0] = (px >> 16) as u8;
        out[1] = (px >> 8) as u8;
        out[2] = *px as u8;
        out[3] = 255;
    }
    let area = gfx::Rect { x: 0.0, y: 0.0, width: RES_X as f32, height: RES_Y as f32 };
    let _ = canvas2d::draw_pixels(h.canvas, area, RES_X as u32, RES_Y as u32, &h.rgba);
    let _ = canvas2d::present(h.canvas);
    let _ = window::request_redraw(h.win);
    h.frames += 1;
}

#[no_mangle]
pub extern "C" fn DG_SleepMs(ms: u32) {
    sleep::sleep_millis(ms);
}

#[no_mangle]
pub extern "C" fn DG_GetTicksMs() -> u32 {
    (clock::monotonic_nanos() / 1_000_000) as u32
}

#[no_mangle]
pub unsafe extern "C" fn DG_GetKey(pressed: *mut c_int, key: *mut c_uchar) -> c_int {
    let h = host();
    if h.queue.is_empty() {
        return 0;
    }
    let (down, code) = h.queue.remove(0);
    *pressed = down as c_int;
    *key = code;
    1
}

#[no_mangle]
pub extern "C" fn DG_SetWindowTitle(_title: *const c_char) {}

// ---- what c/libc.c hands over --------------------------------------------

const HEADER: usize = 16;

#[no_mangle]
pub unsafe extern "C" fn krate_c_malloc(n: usize) -> *mut c_void {
    let layout = Layout::from_size_align_unchecked(n + HEADER, 16);
    let p = alloc(layout);
    if p.is_null() {
        return p as *mut c_void;
    }
    *(p as *mut usize) = n;
    p.add(HEADER) as *mut c_void
}

#[no_mangle]
pub unsafe extern "C" fn krate_c_free(p: *mut c_void) {
    let base = (p as *mut u8).sub(HEADER);
    let n = *(base as *const usize);
    dealloc(base, Layout::from_size_align_unchecked(n + HEADER, 16));
}

#[no_mangle]
pub unsafe extern "C" fn krate_c_realloc(p: *mut c_void, n: usize) -> *mut c_void {
    let base = (p as *mut u8).sub(HEADER);
    let old = *(base as *const usize);
    let q = realloc(base, Layout::from_size_align_unchecked(old + HEADER, 16), n + HEADER);
    if q.is_null() {
        return q as *mut c_void;
    }
    *(q as *mut usize) = n;
    q.add(HEADER) as *mut c_void
}

#[no_mangle]
pub unsafe extern "C" fn krate_c_write(_stream: c_int, p: *const c_char, n: usize) {
    let bytes = core::slice::from_raw_parts(p as *const u8, n);
    let _ = stdio::stdout().write(bytes);
}

#[no_mangle]
pub unsafe extern "C" fn krate_c_wad(len: *mut usize) -> *const u8 {
    *len = WAD.len();
    WAD.as_ptr()
}

#[no_mangle]
pub extern "C" fn krate_c_exit(code: c_int) -> ! {
    let _ = stdio::stdout().write(b"\ndoom: exit\n");
    if let Some(h) = unsafe { HOST.as_ref() } {
        let _ = window::close(h.win);
    }
    let _ = code;
    core::arch::wasm32::unreachable()
}

// ---- the app ---------------------------------------------------------------

fn node(id: u64, parent: Option<u64>, kind: types::WidgetKind) -> types::WidgetNode {
    types::WidgetNode {
        id,
        parent,
        kind,
        label: None,
        role: None,
        style: types::Style {
            width: None,
            height: None,
            grow: 1.0,
            padding: 0.0,
            text: None,
            place: None,
            box_: None,
        },
        checked: None,
        value: None,
        selected: None,
        text_cursor: None,
    }
}

struct Component;

impl krate::Guest for Component {
    fn run() -> i32 {
        let raw = args::raw();
        let quick = raw.split_whitespace().any(|w| w == "quick");
        // `bench`: a real window, Doom's own attract-mode demo, 20 seconds,
        // then the frame rate -- the number that says whether it plays.
        let bench = raw.split_whitespace().any(|w| w == "bench");
        let Ok(win) = window::create("Doom", types::WindowSize { width: 960, height: 600 }) else {
            return 1;
        };
        if tree::set_root(win, &node(ROOT_ID, None, types::WidgetKind::Stack)).is_err() {
            return 1;
        }
        let _ = tree::upsert_node(win, &node(CANVAS_ID, Some(ROOT_ID), types::WidgetKind::Canvas));
        let Ok(canvas) = canvas2d::bind(win, CANVAS_ID) else {
            return 1;
        };
        // Draw in Doom's own 320x200; the host scales it to the window.
        let _ = canvas2d::set_design_size(canvas, gfx::Size { width: RES_X as f32, height: RES_Y as f32 });
        unsafe {
            HOST = Some(Host {
                win,
                canvas,
                rgba: alloc::vec![0; RES_X * RES_Y * 4],
                frames: 0,
                held: [false; KEYS.len()],
                queue: Vec::new(),
            });
        }

        let mut argv_words: [&[u8]; 3] = [b"doom\0", b"-iwad\0", b"doom1.wad\0"];
        let mut argv: [*mut c_char; 4] = [
            argv_words[0].as_ptr() as *mut c_char,
            argv_words[1].as_ptr() as *mut c_char,
            argv_words[2].as_ptr() as *mut c_char,
            core::ptr::null_mut(),
        ];
        let _ = &mut argv_words;
        unsafe { doomgeneric_Create(3, argv.as_mut_ptr()) };

        let started = clock::monotonic_nanos();
        let mut ticks: u64 = 0;
        loop {
            if !quick {
                match events::wait(Some(1)) {
                    Some(types::Event::CloseRequested(_)) => break,
                    _ => {}
                }
                poll_keys();
            }
            unsafe { doomgeneric_Tick() };
            ticks += 1;
            if quick && ticks >= 300 {
                break;
            }
            if bench && clock::monotonic_nanos() - started > 20_000_000_000 {
                break;
            }
        }
        let secs = (clock::monotonic_nanos() - started) as f64 / 1e9;
        let h = host();
        let _ = stdio::stdout().write(alloc::format!(
            "\nticks:{ticks} frames:{} seconds:{secs:.1} fps:{:.1}\n",
            h.frames,
            h.frames as f64 / secs
        ).as_bytes());
        let _ = window::close(win);
        0
    }
}

krate::export!(Component);
