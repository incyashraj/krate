//! The verifier against APKs signed by Android's own apksigner (see
//! tests/fixtures/README.md): what it accepts and the signer it names must
//! be what apksigner says.

use krate_apksig::challenge::{check_answer, statement_in, NotProved};
use krate_apksig::{verify, Refused, Scheme};

const RSA: &str = "19d4c337b50b2e946cb268adfe217d4212f5d25e111aa32e73341a17b65622fb";
const EC: &str = "cd5e303f0093186e8fd20b6824526d888d9c8a3b7822d3d251c5c17bda98bbad";
const OLD: &str = "ca485378b746c9dcb525c9ea6ca7fa9af19bd64e8aedf259a3f47505c1d398a4";
const NEW: &str = "f7dd875d0b5102500131253c1090b57cf7c3558478444fff3d581c935c10985d";

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!(
        "{}/tests/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

#[test]
fn apksigner_rsa_and_ec_signatures_verify_as_apksigner_says() {
    for (file, cert) in [("app-rsa.apk", RSA), ("app-ec.apk", EC)] {
        let verified = verify(&fixture(file)).unwrap_or_else(|e| panic!("{file}: {e}"));
        assert_eq!(
            verified.package.as_deref(),
            Some("com.example.owned"),
            "{file}"
        );
        assert_eq!(verified.scheme, Scheme::V3, "{file}");
        assert_eq!(verified.certificate_sha256, cert, "{file}");
    }
}

#[test]
fn apksigner_key_rotation_names_the_new_key_and_its_lineage() {
    for file in ["app-rotated.apk", "app-rotated-v3.apk"] {
        let verified = verify(&fixture(file)).unwrap_or_else(|e| panic!("{file}: {e}"));
        assert_eq!(
            verified.certificate_sha256, NEW,
            "{file}: the key in charge now"
        );
        assert_eq!(
            verified.lineage,
            vec![OLD.to_string(), NEW.to_string()],
            "{file}"
        );
    }
}

/// Portable test 1715, on apksigner's bytes.
#[test]
fn a_byte_changed_in_an_apksigner_apk_breaks_it() {
    let apk = fixture("app-rsa.apk");
    let at = apk
        .windows(5)
        .position(|w| w == b"dex\n0")
        .expect("the stored dex");
    let mut changed = apk.clone();
    changed[at + 12] ^= 1;
    assert!(matches!(verify(&changed), Err(Refused::Invalid(_))));
}

/// Portable test 1717, answered with apksigner.
#[test]
fn a_challenge_signed_with_apksigner_proves_the_key() {
    let app = verify(&fixture("app-rsa.apk")).unwrap();
    let answer = fixture("challenge-rsa.apk");
    let issued = statement_in(&answer).unwrap();
    assert_eq!(issued.certificate_sha256, app.certificate_sha256);
    assert_eq!(issued.apk_sha256, app.apk_sha256);
    assert!(check_answer(&answer, &issued, 1_800_000_000).is_ok());
    assert_eq!(
        check_answer(&answer, &issued, 4_102_444_801),
        Err(NotProved::Expired)
    );
    match check_answer(&fixture("challenge-wrong-key.apk"), &issued, 1) {
        Err(NotProved::WrongKey {
            signed_with,
            expected,
        }) => {
            assert_eq!((signed_with.as_str(), expected.as_str()), (EC, RSA));
        }
        other => panic!("{other:?}"),
    }
}
