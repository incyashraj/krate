// A Krate guest is no_std: the SDK owns the allocator, panic handler, and mem
// intrinsics, so this app cannot pull std's latent wasi:* imports. The proof
// this app carries is that a real ecosystem crate -- `rand`, which depends on
// getrandom -- works here without the app wiring any entropy source itself. The
// SDK's getrandom backend routes every draw to krate:random/bytes.
#![no_std]
extern crate alloc;

use alloc::string::String;
use krate::{
    io::{args, stdio, streams::OutputStreamExt},
    Guest,
};
use rand::{Rng, SeedableRng};

struct Component;

impl Guest for Component {
    fn run() -> i32 {
        let stdout = stdio::stdout();

        // How many dice to roll: the first argument, default 5. Kept tiny so
        // the app has no reason to touch anything but stdout and entropy.
        let count: u32 = args::raw()
            .split('\n')
            .next()
            .and_then(|first| first.trim().parse().ok())
            .unwrap_or(5);

        // Seed rand's generator from the OS entropy source. On this target that
        // source is getrandom, which the SDK backend satisfies from the host's
        // krate:random/bytes. No OS, no wasi, no per-app wiring -- an unmodified
        // crate getting real entropy through Krate's capability.
        let mut rng = rand::rngs::SmallRng::from_os_rng();

        let mut line = String::new();
        for _ in 0..count {
            let face = rng.random_range(1..=6);
            if !line.is_empty() {
                line.push(' ');
            }
            push_u32(&mut line, face);
        }

        let _ = stdout.write_line(&line);
        let _ = stdout.flush();
        0
    }
}

/// Append a small integer to a string without `format!` (which can pull a
/// panic path). Handles the single-digit dice faces this app produces.
fn push_u32(out: &mut String, mut value: u32) {
    if value == 0 {
        out.push('0');
        return;
    }
    let mut digits = [0u8; 10];
    let mut len = 0;
    while value > 0 {
        digits[len] = b'0' + (value % 10) as u8;
        value /= 10;
        len += 1;
    }
    for i in (0..len).rev() {
        out.push(digits[i] as char);
    }
}

krate::export!(Component);
