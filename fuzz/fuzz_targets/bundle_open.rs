#![no_main]

//! Feed arbitrary bytes to the bundle opener (IC-209).
//!
//! This is the one parser in Krate that reads a file a stranger sent. It
//! decides how many entries there are, how big they are, where they may be
//! written, and whether two of them name the same file -- all from numbers
//! the archive itself supplies. Every one of those numbers can be a lie, and
//! several of the defects found by hand (K-252, K-255, K-257) were exactly
//! that: a header that said one thing while the bytes said another.
//!
//! The contract being fuzzed is narrow and total: for ANY input, open_reader
//! either returns a bundle or returns an error. It must not panic, and it
//! must not hang. Nothing here asserts a particular verdict -- a fuzzer
//! cannot know which arbitrary bytes are a valid bundle -- so what it proves
//! is the absence of a crash on the path that reads untrusted input.

use libfuzzer_sys::fuzz_target;
use std::io::Cursor;

fuzz_target!(|data: &[u8]| {
    // Cursor rather than a temp file: the fuzzer runs this millions of times
    // and touching the disk each round would make it useless.
    let _ = krate_bundle::open_reader(Cursor::new(data));
});
