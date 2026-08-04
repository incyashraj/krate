//! HTTPS must actually work, not just be wired up.
//!
//! `Plan/HTTPS-Gap-2026-08-01.md` records that a Krate app could fetch
//! `http://` but not `https://`: the hand-written framing spoke plaintext, so
//! an https URL connected to port 443 and then said "GET / HTTP/1.1" to a
//! server waiting for a handshake. Every API worth calling was unreachable.
//!
//! The fix routes TLS through ureq (`fetch_over_tls`), but nothing tested it,
//! so the claim "FIXED" rested on one manual run. A reviewer reading the code
//! could reasonably conclude TLS was absent -- there is no rustls or
//! native-tls in any Cargo.toml, because ureq carries it.
//!
//! Ignored by default: it needs the network, and a test that fails on a plane
//! is a test people learn to skip. Run it deliberately:
//!
//!     cargo test -p krate-runtime --test https_reaches_a_real_host -- --ignored

#[test]
#[ignore = "needs network access"]
fn https_completes_a_real_handshake() {
    let response = ureq::AgentBuilder::new()
        .redirects(0)
        .build()
        .get("https://example.com")
        .call();

    match response {
        Ok(response) => assert_eq!(
            response.status(),
            200,
            "example.com should answer 200 over TLS"
        ),
        Err(error) => panic!(
            "HTTPS failed, so Krate apps cannot reach any real API: {error}\n\
             If this is a network-less machine, that is the cause. If not, the \
             TLS path in crates/runtime/src/lib.rs (fetch_over_tls) regressed."
        ),
    }
}

/// The plaintext path must keep working too -- the TLS branch is chosen on the
/// URL scheme, and a mistake there breaks http:// rather than https://.
#[test]
#[ignore = "needs network access"]
fn plain_http_still_works() {
    let response = ureq::AgentBuilder::new()
        .redirects(0)
        .build()
        .get("http://example.com")
        .call();
    assert!(
        response.is_ok(),
        "plain http regressed: {:?}",
        response.err()
    );
}
