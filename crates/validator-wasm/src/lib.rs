//! The bundle validator, exported for a WebAssembly host (IC-833, K-309).
//!
//! The hub is a Cloudflare Worker: JavaScript, with no way to link a Rust
//! crate. So the crate is compiled to a wasm module and the Worker calls
//! [`judge`] with the bytes of an upload. What runs at the hub's door is
//! then `krate_bundle::judge_bytes` -- the same rules, the same code, the
//! same test that holds it to `open` -- and not a JavaScript reading of
//! them. Build it with `scripts/build-hub-validator.sh`.
//!
//! The ABI is three functions over the module's own memory: the host
//! allocates a buffer with [`krate_alloc`], writes the bytes, calls
//! [`krate_judge`], reads the length-prefixed JSON answer it returns, and
//! frees both with [`krate_free`]. The names carry a prefix because a
//! `#[no_mangle] free` on the host build IS the C library's `free`, and
//! the allocator calling into it recursed until the stack ran out.

use std::alloc::{alloc as raw_alloc, dealloc, Layout};

/// Verification never needs randomness; a request for it is a bug, and it
/// is answered as one rather than with bytes that are not random.
fn no_randomness_here(_dest: &mut [u8]) -> Result<(), getrandom::Error> {
    Err(getrandom::Error::UNSUPPORTED)
}
getrandom::register_custom_getrandom!(no_randomness_here);

/// The ceiling on what the entries may expand to together while being
/// judged, in bytes. A Worker has 128 MiB; the archive itself is at most
/// 5 MiB at this door, so this is generous for any real app and small
/// enough that a bomb is refused before it costs anything.
pub const MAX_EXPANDED_BYTES: u64 = 64 * 1024 * 1024;

/// Judge `bytes` and answer as JSON: `{"ok":true,"judgement":{...}}` or
/// `{"ok":false,"problem":"..."}`. The problem text is `BundleError`'s own
/// words, the ones `krate run` would print for the same file.
pub fn judge_to_json(bytes: &[u8]) -> String {
    match krate_bundle::judge_bytes(bytes, MAX_EXPANDED_BYTES) {
        Ok(judgement) => serde_json::json!({ "ok": true, "judgement": judgement }).to_string(),
        Err(err) => serde_json::json!({ "ok": false, "problem": err.to_string() }).to_string(),
    }
}

/// Reserve `len` bytes in the module's memory for the host to write into.
///
/// # Safety
/// The host owns the returned region until it passes it back to
/// [`krate_free`] with the same length.
#[no_mangle]
pub unsafe extern "C" fn krate_alloc(len: u32) -> *mut u8 {
    if len == 0 {
        return std::ptr::NonNull::<u8>::dangling().as_ptr();
    }
    let layout = Layout::from_size_align(len as usize, 1).expect("layout");
    raw_alloc(layout)
}

/// Release a region [`krate_alloc`] or [`krate_judge`] handed out.
///
/// # Safety
/// `ptr` and `len` must be exactly what was handed out.
#[no_mangle]
pub unsafe extern "C" fn krate_free(ptr: *mut u8, len: u32) {
    if len == 0 {
        return;
    }
    let layout = Layout::from_size_align(len as usize, 1).expect("layout");
    dealloc(ptr, layout);
}

/// Judge the `len` bytes at `ptr`. Returns a pointer to a buffer holding a
/// little-endian u32 length followed by that many bytes of JSON; free it
/// with [`krate_free`] and `4 + length`.
///
/// # Safety
/// `ptr`/`len` must describe a region the host wrote through
/// [`krate_alloc`].
#[no_mangle]
pub unsafe extern "C" fn krate_judge(ptr: *const u8, len: u32) -> *mut u8 {
    let bytes = std::slice::from_raw_parts(ptr, len as usize);
    let json = judge_to_json(bytes).into_bytes();
    let total = 4 + json.len();
    let layout = Layout::from_size_align(total, 1).expect("layout");
    let out = raw_alloc(layout);
    std::ptr::copy_nonoverlapping((json.len() as u32).to_le_bytes().as_ptr(), out, 4);
    std::ptr::copy_nonoverlapping(json.as_ptr(), out.add(4), json.len());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The JSON the host reads carries the judgement or the refusal, in
    /// the words the CLI uses.
    #[test]
    fn the_answer_is_the_judgement_or_the_refusal() {
        let bounce = include_bytes!("../../../evidence/ported/bounce.krate");
        let answer: serde_json::Value = serde_json::from_str(&judge_to_json(bounce)).unwrap();
        assert_eq!(answer["ok"], true, "{answer}");
        assert_eq!(
            answer["judgement"]["manifest"]["app"]["id"], "dev.krate.bounce",
            "{answer}"
        );
        assert_eq!(answer["judgement"]["signature"]["state"], "absent");
        assert_eq!(
            answer["judgement"]["execution"].as_str().map(str::len),
            Some(64)
        );

        let answer: serde_json::Value =
            serde_json::from_str(&judge_to_json(b"PK\x03\x04 not an archive")).unwrap();
        assert_eq!(answer["ok"], false);
        assert!(
            answer["problem"]
                .as_str()
                .unwrap()
                .contains("not a valid .krate bundle"),
            "{answer}"
        );
    }

    /// The raw ABI round-trips: what `judge` writes is what `judge_to_json`
    /// says, length-prefixed, and both buffers free cleanly.
    #[test]
    fn the_abi_round_trips() {
        let bounce = include_bytes!("../../../evidence/ported/bounce.krate");
        unsafe {
            let input = krate_alloc(bounce.len() as u32);
            std::ptr::copy_nonoverlapping(bounce.as_ptr(), input, bounce.len());
            let out = krate_judge(input, bounce.len() as u32);
            let len = u32::from_le_bytes(*(out as *const [u8; 4])) as usize;
            let json = std::slice::from_raw_parts(out.add(4), len).to_vec();
            krate_free(out, (4 + len) as u32);
            krate_free(input, bounce.len() as u32);
            assert_eq!(String::from_utf8(json).unwrap(), judge_to_json(bounce));
        }
    }
}
