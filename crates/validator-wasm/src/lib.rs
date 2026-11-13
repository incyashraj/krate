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

/// What a browser tab that is an owner's device asks of the module
/// (IC-958): signing, and the organisation's keyring and signing ring.
mod browser;
pub use browser::{delegation_to_json, keys_to_json, sign_frame};

/// An organisation's team data in a browser tab (store.org, G1): the
/// device's copy, the merge and the sealing a computer runs.
mod teamdata;
pub use teamdata::org_data_to_json;

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
pub const MAX_EXPANDED_BYTES: u64 = krate_bundle::HUB_MAX_EXPANDED_BYTES;

/// Judge `bytes` and answer as JSON: `{"ok":true,"judgement":{...}}` or
/// `{"ok":false,"problem":"..."}`. The problem text is `BundleError`'s own
/// words, the ones `krate run` would print for the same file.
pub fn judge_to_json(bytes: &[u8]) -> String {
    krate_bundle::judgement_json(krate_bundle::judge_bytes(bytes, MAX_EXPANDED_BYTES))
}

/// Judge `bytes` for the browser Player (IC-956): the same judgement as
/// [`judge_to_json`], plus `modules`, the SHA-256 of every core module
/// embedded in the judged component, in order. The tab runs a `.wasm` file
/// only when its digest is one of these, so what runs in a tab is the code
/// this judgement was about, whoever served the files.
pub fn judge_for_tab_to_json(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let refused =
        |problem: String| serde_json::json!({ "ok": false, "problem": problem }).to_string();
    let (judgement, entries) =
        match krate_bundle::judge_bytes_and_entries(bytes, MAX_EXPANDED_BYTES) {
            Ok(judged) => judged,
            Err(err) => return refused(err.to_string()),
        };
    let Some(component) = entries.get(krate_bundle::COMPONENT_ENTRY) else {
        return refused(format!(
            "the bundle has no {}",
            krate_bundle::COMPONENT_ENTRY
        ));
    };
    let mut modules = Vec::new();
    for payload in wasmparser::Parser::new(0).parse_all(component) {
        match payload {
            Ok(wasmparser::Payload::ModuleSection {
                unchecked_range, ..
            }) => match component.get(unchecked_range) {
                Some(slice) => modules.push(
                    Sha256::digest(slice)
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect::<String>(),
                ),
                None => {
                    return refused("a core module runs past the end of the component".to_string())
                }
            },
            Ok(_) => {}
            Err(err) => return refused(format!("the component does not parse: {err}")),
        }
    }
    serde_json::json!({ "ok": true, "judgement": judgement, "modules": modules }).to_string()
}

/// Read a revocation list written by `krate revoke` and answer as JSON:
/// `{"ok":true,"root":"<hex>","issued_at":n,"revocations":[...]}` once the
/// root it names has been checked to have signed it, or
/// `{"ok":false,"problem":"..."}` (IC-946). The hub's organisation store
/// keeps the keys an owner withdrew from this; a list nobody signed is
/// not a list.
pub fn revocations_to_json(bytes: &[u8]) -> String {
    use krate_bundle::delegation::SignedRevocationList;
    let refused =
        |problem: String| serde_json::json!({ "ok": false, "problem": problem }).to_string();
    let signed: SignedRevocationList = match serde_json::from_slice(bytes) {
        Ok(signed) => signed,
        Err(err) => {
            return refused(format!(
                "not a revocation list written by `krate revoke`: {err}"
            ))
        }
    };
    match signed.verify() {
        Ok(list) => serde_json::json!({
            "ok": true,
            "root": list.root,
            "issued_at": list.issued_at,
            "revocations": list.revocations.iter().map(|r| serde_json::json!({
                "key": r.key,
                "compromised_from": r.compromised_from,
                "reason": r.reason,
            })).collect::<Vec<_>>(),
        })
        .to_string(),
        Err(err) => refused(err.to_string()),
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
    answer(judge_to_json(bytes))
}

/// [`judge_for_tab_to_json`] over the `len` bytes at `ptr`, answered the
/// way [`krate_judge`] answers.
///
/// # Safety
/// As [`krate_judge`].
#[no_mangle]
pub unsafe extern "C" fn krate_judge_for_tab(ptr: *const u8, len: u32) -> *mut u8 {
    let bytes = std::slice::from_raw_parts(ptr, len as usize);
    answer(judge_for_tab_to_json(bytes))
}

/// The component of a bundle that passes judgement, for the hub's publish
/// step that makes what a browser tab runs (IC-956): `Ok(code.wasm)` exactly
/// as judged, or the refusal in the validator's words.
pub fn judged_component(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let (_, mut entries) = krate_bundle::judge_bytes_and_entries(bytes, MAX_EXPANDED_BYTES)
        .map_err(|err| err.to_string())?;
    entries
        .remove(krate_bundle::COMPONENT_ENTRY)
        .ok_or_else(|| format!("the bundle has no {}", krate_bundle::COMPONENT_ENTRY))
}

/// [`judged_component`] over the `len` bytes at `ptr`: a little-endian u32
/// length, then one byte (1 for the component, 0 for a refusal), then the
/// component's bytes or the refusal's words. Free with [`krate_free`] and
/// `4 + length`.
///
/// # Safety
/// As [`krate_judge`].
#[no_mangle]
pub unsafe extern "C" fn krate_judged_component(ptr: *const u8, len: u32) -> *mut u8 {
    let bytes = std::slice::from_raw_parts(ptr, len as usize);
    let body = match judged_component(bytes) {
        Ok(component) => [vec![1u8], component].concat(),
        Err(problem) => [vec![0u8], problem.into_bytes()].concat(),
    };
    let total = 4 + body.len();
    let layout = Layout::from_size_align(total, 1).expect("layout");
    let out = raw_alloc(layout);
    std::ptr::copy_nonoverlapping((body.len() as u32).to_le_bytes().as_ptr(), out, 4);
    std::ptr::copy_nonoverlapping(body.as_ptr(), out.add(4), body.len());
    out
}

/// [`revocations_to_json`] over the `len` bytes at `ptr`, answered the way
/// [`krate_judge`] answers.
///
/// # Safety
/// As [`krate_judge`].
#[no_mangle]
pub unsafe extern "C" fn krate_revocations(ptr: *const u8, len: u32) -> *mut u8 {
    let bytes = std::slice::from_raw_parts(ptr, len as usize);
    answer(revocations_to_json(bytes))
}

/// [`keys_to_json`] over the JSON at `ptr`: one keyring or signing-ring
/// operation for a browser that is an owner's device, answered the way
/// [`krate_judge`] answers.
///
/// # Safety
/// As [`krate_judge`].
#[no_mangle]
pub unsafe extern "C" fn krate_keys(ptr: *const u8, len: u32) -> *mut u8 {
    let bytes = std::slice::from_raw_parts(ptr, len as usize);
    answer(keys_to_json(bytes))
}

/// [`sign_frame`] over the frame at `ptr`: mark and sign a bundle. The
/// answer is length-prefixed like [`krate_judge`]'s, and is itself a frame
/// (a u32 JSON length, the JSON, the signed bundle).
///
/// # Safety
/// As [`krate_judge`].
#[no_mangle]
pub unsafe extern "C" fn krate_sign(ptr: *const u8, len: u32) -> *mut u8 {
    let bytes = std::slice::from_raw_parts(ptr, len as usize);
    answer_bytes(sign_frame(bytes))
}

/// [`org_data_to_json`] over the JSON at `ptr`: one team-data call for a
/// browser tab, answered the way [`krate_judge`] answers.
///
/// # Safety
/// As [`krate_judge`].
#[no_mangle]
pub unsafe extern "C" fn krate_org_data(ptr: *const u8, len: u32) -> *mut u8 {
    let bytes = std::slice::from_raw_parts(ptr, len as usize);
    answer(org_data_to_json(bytes))
}

/// [`delegation_to_json`] over the JSON at `ptr`: the hub's reading of a
/// permission slip a device signed.
///
/// # Safety
/// As [`krate_judge`].
#[no_mangle]
pub unsafe extern "C" fn krate_delegation(ptr: *const u8, len: u32) -> *mut u8 {
    let bytes = std::slice::from_raw_parts(ptr, len as usize);
    answer(delegation_to_json(bytes))
}

/// A length-prefixed copy of `json` in the module's memory.
unsafe fn answer(json: String) -> *mut u8 {
    answer_bytes(json.into_bytes())
}

/// A length-prefixed copy of `json` (any bytes) in the module's memory.
unsafe fn answer_bytes(json: Vec<u8>) -> *mut u8 {
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

    /// The tab's judgement is the hub's, plus the digest of each core module
    /// of the judged component; a refusal is the same refusal.
    #[test]
    fn the_tab_judgement_adds_the_core_module_digests() {
        let bounce = include_bytes!("../../../evidence/ported/bounce.krate");
        let tab: serde_json::Value = serde_json::from_str(&judge_for_tab_to_json(bounce)).unwrap();
        let hub: serde_json::Value = serde_json::from_str(&judge_to_json(bounce)).unwrap();
        assert_eq!(tab["judgement"], hub["judgement"]);
        let modules = tab["modules"].as_array().unwrap();
        assert!(!modules.is_empty());
        assert!(modules.iter().all(|m| m.as_str().unwrap().len() == 64));
        let refused: serde_json::Value =
            serde_json::from_str(&judge_for_tab_to_json(b"PK\x03\x04 not an archive")).unwrap();
        assert_eq!(refused["ok"], false);
    }

    /// The hub's publish step gets the component it judged, or the refusal.
    #[test]
    fn the_judged_component_is_the_bundles_own() {
        let bounce = include_bytes!("../../../evidence/ported/bounce.krate");
        let component = judged_component(bounce).expect("judged");
        assert_eq!(&component[..4], b"\0asm");
        assert!(judged_component(b"not a bundle").is_err());
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
