//! Proof that an ordinary C++ program runs as a Krate app: cpp/probe.cpp is
//! built with wasi-sdk, and krate-c-runtime answers its WASI calls.
#![no_std]
extern crate alloc;

use core::ffi::{c_char, c_int};
use krate::bindings::krate::io::stdio;

extern "C" {
    fn probe_main(argc: c_int, argv: *mut *mut c_char) -> c_int;
}

struct Component;

impl krate::Guest for Component {
    fn run() -> i32 {
        krate_c_runtime::set_args(&["probe", "from-krate"]);
        krate_c_runtime::start();
        let mut a0 = *b"probe\0";
        let mut a1 = *b"from-krate\0";
        let mut argv = [
            a0.as_mut_ptr() as *mut c_char,
            a1.as_mut_ptr() as *mut c_char,
            core::ptr::null_mut(),
        ];
        let code = unsafe { probe_main(2, argv.as_mut_ptr()) };
        krate_c_runtime::finish();
        let _ = stdio::stdout().write(alloc::format!("exit: {code}\n").as_bytes());
        code
    }
}

krate::export!(Component);
