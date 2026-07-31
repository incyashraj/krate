//! Random bytes for guest apps, from the operating system.
//!
//! `getrandom` is the third most-downloaded crate in Rust and `rand` and `uuid`
//! sit on top of it, so a great many ordinary programs -- anything shuffling a
//! list, generating an id, or picking a sample -- cannot be ported without
//! this. It was the largest single gap between what apps need and what Krate
//! offered.
//!
//! Every byte comes from the OS entropy pool. There is no seeded generator
//! here on purpose: an app that gets a predictable stream while believing it is
//! random is worse off than one that cannot get random numbers at all.

use std::io;

/// How many bytes one call may ask for.
///
/// A guest asking for a gigabyte of entropy is a bug or an attempt to stall the
/// host; real uses want a few dozen bytes. Callers that legitimately need more
/// can call again.
pub const MAX_BYTES: usize = 64 * 1024;

/// Why a request for random bytes could not be served.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RandomError {
    /// The app did not hold `random.bytes`.
    Denied,
    /// More than `MAX_BYTES` was asked for in one call.
    TooLarge,
    /// The OS entropy source could not be read.
    Unavailable(String),
}

impl std::fmt::Display for RandomError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Denied => write!(f, "this app was not given permission to use random numbers"),
            Self::TooLarge => write!(f, "asked for more than {MAX_BYTES} random bytes at once"),
            Self::Unavailable(why) => write!(f, "no random source available: {why}"),
        }
    }
}

/// Fill `buf` with random bytes from the OS.
///
/// Fails rather than falling back to anything weaker. A caller that receives an
/// error knows it got nothing; a caller handed low-quality bytes it believes are
/// strong has no way to find out.
pub fn fill(buf: &mut [u8]) -> Result<(), RandomError> {
    if buf.len() > MAX_BYTES {
        return Err(RandomError::TooLarge);
    }
    if buf.is_empty() {
        return Ok(());
    }
    os_random(buf).map_err(|err| RandomError::Unavailable(err.to_string()))
}

/// Return `count` random bytes, or an error.
pub fn bytes(count: u32) -> Result<Vec<u8>, RandomError> {
    let count = count as usize;
    if count > MAX_BYTES {
        return Err(RandomError::TooLarge);
    }
    let mut out = vec![0u8; count];
    fill(&mut out)?;
    Ok(out)
}

/// A uniformly distributed `u64`.
pub fn next_u64() -> Result<u64, RandomError> {
    let mut buf = [0u8; 8];
    fill(&mut buf)?;
    Ok(u64::from_le_bytes(buf))
}

/// A uniform integer in `[0, bound)`, or `None` when `bound` is zero.
///
/// Uses rejection sampling rather than `%`. Taking the remainder of a random
/// number skews the result toward the low end whenever `bound` does not divide
/// the range evenly -- a shuffled deck would deal some cards more often than
/// others, and nothing in the output would look wrong.
pub fn below(bound: u64) -> Result<Option<u64>, RandomError> {
    if bound == 0 {
        return Ok(None);
    }
    // The largest multiple of `bound` that fits in a u64. Draws at or above it
    // are discarded so every remaining value maps to exactly one outcome.
    let limit = u64::MAX - (u64::MAX % bound) - 1;
    loop {
        let value = next_u64()?;
        if value <= limit {
            return Ok(Some(value % bound));
        }
    }
}

#[cfg(unix)]
fn os_random(buf: &mut [u8]) -> io::Result<()> {
    use std::io::Read;
    // `/dev/urandom` is the right source on every Unix Krate targets: it is
    // seeded once at boot and never blocks afterwards.
    std::fs::File::open("/dev/urandom")?.read_exact(buf)
}

#[cfg(windows)]
fn os_random(buf: &mut [u8]) -> io::Result<()> {
    // `RtlGenRandom` is Windows' entropy source. Declared directly rather than
    // pulling in a dependency for one call. It is exported from advapi32 under
    // the name `SystemFunction036`, which is the documented way to reach it.
    #[link(name = "advapi32")]
    unsafe extern "system" {
        #[link_name = "SystemFunction036"]
        fn rtl_gen_random(buffer: *mut u8, length: u32) -> u8;
    }

    // Chunked because the length is a u32; MAX_BYTES fits, but this keeps the
    // conversion honest rather than relying on the cap staying small.
    for chunk in buf.chunks_mut(u32::MAX as usize) {
        let ok = unsafe { rtl_gen_random(chunk.as_mut_ptr(), chunk.len() as u32) };
        if ok == 0 {
            return Err(io::Error::other("RtlGenRandom failed"));
        }
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn os_random(buf: &mut [u8]) -> io::Result<()> {
    let _ = buf;
    // Refuse rather than invent entropy. Anything this could synthesize would
    // be guessable, and an app cannot tell the difference.
    Err(io::Error::other("unsupported platform"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_returns_the_number_of_bytes_asked_for() {
        assert_eq!(bytes(0).expect("zero").len(), 0);
        assert_eq!(bytes(1).expect("one").len(), 1);
        assert_eq!(bytes(32).expect("32").len(), 32);
    }

    #[test]
    fn the_bytes_are_not_a_fixed_pattern() {
        // Not a statistical test -- just proof that a real source was read and
        // the buffer was not left as the zeros it was allocated with.
        let a = bytes(64).expect("draw");
        let b = bytes(64).expect("draw");
        assert_ne!(a, b, "two draws returned identical bytes");
        assert!(a.iter().any(|&x| x != 0), "returned all zeros");
    }

    #[test]
    fn an_unreasonable_request_is_refused() {
        let err = bytes(MAX_BYTES as u32 + 1).expect_err("must refuse");
        assert_eq!(err, RandomError::TooLarge);
        // And the refusal says what the limit is, so a caller can adjust.
        assert!(err.to_string().contains(&MAX_BYTES.to_string()));
    }

    #[test]
    fn below_stays_in_range_and_handles_zero() {
        assert_eq!(below(0).expect("zero bound"), None);
        for bound in [1u64, 2, 6, 52, 1000] {
            for _ in 0..200 {
                let value = below(bound).expect("draw").expect("some");
                assert!(value < bound, "{value} out of range for bound {bound}");
            }
        }
    }

    #[test]
    fn below_does_not_favour_low_values() {
        // The reason for rejection sampling. With `% bound` on a skewed range
        // the low buckets come up more often; over this many draws an obvious
        // bias would show. Bounds are loose so the test does not flake.
        let bound = 6u64;
        let draws = 6000;
        let mut counts = [0u32; 6];
        for _ in 0..draws {
            let value = below(bound).expect("draw").expect("some") as usize;
            counts[value] += 1;
        }
        let expected = draws / bound as u32;
        for (face, &count) in counts.iter().enumerate() {
            assert!(
                count > expected / 2 && count < expected * 2,
                "face {face} came up {count} times, expected near {expected}"
            );
        }
    }

    #[test]
    fn a_large_but_allowed_request_is_served_whole() {
        let out = bytes(MAX_BYTES as u32).expect("max request");
        assert_eq!(out.len(), MAX_BYTES);
        // A short read that silently returned fewer bytes would be worse than
        // an error, so check the tail was actually written.
        assert!(out[MAX_BYTES - 32..].iter().any(|&x| x != 0));
    }
}
