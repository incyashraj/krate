#![cfg_attr(target_arch = "wasm32", no_std)]
#![allow(dead_code)]
extern crate alloc;
#[cfg(target_arch = "wasm32")]
extern crate krate;
#[cfg(target_arch = "wasm32")]
#[allow(warnings)]
mod bindings;
#[cfg(test)]
extern crate std;
mod colors;
#[cfg(target_arch = "wasm32")]
mod guest;
#[cfg(target_arch = "wasm32")]
mod guest_io;
mod hexyl;
mod options;
#[cfg(target_arch = "wasm32")]
mod viewer;
#[cfg(target_arch = "wasm32")]
struct Component;
#[cfg(target_arch = "wasm32")]
impl bindings::Guest for Component {
    fn run() -> i32 {
        let raw = bindings::krate::io::args::raw();
        let args: alloc::vec::Vec<&str> = raw.split('\n').filter(|a| !a.is_empty()).collect();
        if args.first() == Some(&"--dump") {
            guest::dump(&args[1..])
        } else {
            viewer::run(&args)
        }
    }
}
#[cfg(target_arch = "wasm32")]
bindings::export!(Component with_types_in bindings);
