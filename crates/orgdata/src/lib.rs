//! An organisation's team data on one device (`store.org`, IC-951).
//!
//! Two parts, both pure -- no disk, no clock, no network:
//!
//! - [`merge`]: which write wins. The hub applies the same rule
//!   (`cloud/worker/src/org-data-merge.js`) and both pass one set of vectors.
//! - [`mirror`]: the device's copy of one app's data -- writing, reading,
//!   paging, what one sync sends and how its answer is folded in, and the
//!   sealing of what leaves the device (feature `seal`).
//!
//! A computer runs this inside `krate-runtime` (`org_store`, which adds the
//! file, the background sync and the hub's keyring protocol). A browser tab
//! runs the very same code compiled into the hub's validator module
//! (`crates/validator-wasm`), with the Player page doing the storage and the
//! network. One implementation, so a tab and a computer cannot disagree.

pub mod merge;
pub mod mirror;

/* ---- base64 (standard, padded): encoding, not cryptography ------------- */

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with padding: the `{"x": ...}` wire form of bytes.
pub fn b64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            B64[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            B64[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// Read base64 the way the runtime always has for team data: padding and
/// line breaks ignored, anything outside the alphabet refused.
pub fn b64_decode(text: &str) -> Option<Vec<u8>> {
    let value = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => u32::from(c - b'A'),
            b'a'..=b'z' => u32::from(c - b'a') + 26,
            b'0'..=b'9' => u32::from(c - b'0') + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        })
    };
    let clean: Vec<u8> = text.bytes().filter(|&c| c != b'=' && c != b'\n').collect();
    let mut out = Vec::with_capacity(clean.len() * 3 / 4);
    for chunk in clean.chunks(4) {
        let mut n: u32 = 0;
        for (i, &c) in chunk.iter().enumerate() {
            n |= value(c)? << (18 - 6 * i as u32);
        }
        out.push((n >> 16) as u8);
        if chunk.len() > 2 {
            out.push((n >> 8) as u8);
        }
        if chunk.len() > 3 {
            out.push(n as u8);
        }
    }
    Some(out)
}
