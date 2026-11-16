use super::challenge::{challenge_apk, check_answer, statement_in, NotProved, Statement};
use super::testsign::{sign, unsigned_apk, Signing, TestKey};
use super::*;

fn signed(key: &TestKey, v3: bool) -> Vec<u8> {
    sign(
        &unsigned_apk("com.example.owned", b"dex\n035\0 the app's code"),
        Signing {
            key,
            v3,
            rotated_from: &[],
            twice: false,
            claim: None,
            forge_links_with: None,
        },
    )
}

fn sha(bytes: &[u8]) -> String {
    hex(digest::digest(&digest::SHA256, bytes).as_ref())
}

#[test]
fn a_signed_apk_verifies_and_names_its_signer() {
    let key = TestKey::new("Owner");
    for v3 in [false, true] {
        let apk = signed(&key, v3);
        let verified = verify(&apk).unwrap();
        assert_eq!(verified.package.as_deref(), Some("com.example.owned"));
        assert_eq!(verified.scheme, if v3 { Scheme::V3 } else { Scheme::V2 });
        assert_eq!(verified.certificate_sha256, sha(&key.certificate));
        assert_eq!(verified.lineage, vec![sha(&key.certificate)]);
        assert_eq!(verified.apk_sha256, sha(&apk));
    }
}

/// Portable test 1715: changing the code after signing breaks the result.
#[test]
fn changing_classes_dex_after_signing_invalidates_it() {
    let key = TestKey::new("Owner");
    for v3 in [false, true] {
        let apk = signed(&key, v3);
        let at = apk
            .windows(5)
            .position(|w| w == b"dex\n0")
            .expect("the dex is stored");
        let mut changed = apk.clone();
        changed[at + 10] ^= 1;
        assert!(
            matches!(verify(&changed), Err(Refused::Invalid(_))),
            "a changed dex"
        );
        // The central directory is covered too: a renamed entry is caught.
        let mut renamed = apk.clone();
        let cd = renamed
            .windows(4)
            .rposition(|w| w == [0x50, 0x4b, 0x01, 0x02])
            .unwrap();
        renamed[cd + 46] ^= 0x20;
        assert!(
            matches!(verify(&renamed), Err(Refused::Invalid(_))),
            "a changed directory"
        );
    }
}

#[test]
fn a_certificate_claimed_with_another_key_is_refused() {
    let owner = TestKey::new("Owner");
    let thief = TestKey::new("Thief");
    // Everything signed correctly by the thief's key, naming the owner's
    // certificate: only the key-matches-certificate check stops it.
    let forged = sign(
        &unsigned_apk("com.example.owned", b"dex\n035\0"),
        Signing {
            key: &thief,
            v3: true,
            rotated_from: &[],
            twice: false,
            claim: Some(&owner.certificate),
            forge_links_with: None,
        },
    );
    match verify(&forged) {
        Err(Refused::Invalid(why)) => assert!(
            why.contains("not the key in the signer's certificate"),
            "{why}"
        ),
        other => panic!("a forged signer was accepted: {other:?}"),
    }
}

/// The theft a lineage would allow if its links were not checked: a thief
/// claims the owner's key rotated to theirs, then answers the owner's
/// challenge with their own key.
#[test]
fn a_rotation_the_old_key_never_signed_is_refused() {
    let owner = TestKey::new("Owner");
    let thief = TestKey::new("Thief");
    let forged = sign(
        &challenge_apk(&issued(&owner.certificate, &signed(&owner, true))),
        Signing {
            key: &thief,
            v3: true,
            rotated_from: &[&owner],
            twice: false,
            claim: None,
            forge_links_with: Some(&thief),
        },
    );
    match verify(&forged) {
        Err(Refused::Invalid(why)) => assert!(why.contains("key rotation"), "{why}"),
        other => panic!("a forged rotation was accepted: {other:?}"),
    }
}

#[test]
fn a_rotation_lineage_is_walked_link_by_link() {
    let old = TestKey::new("Old key");
    let new = TestKey::new("New key");
    let apk = sign(
        &unsigned_apk("com.example.owned", b"dex\n035\0"),
        Signing {
            key: &new,
            v3: true,
            rotated_from: &[&old],
            twice: false,
            claim: None,
            forge_links_with: None,
        },
    );
    let verified = verify(&apk).unwrap();
    assert_eq!(verified.certificate_sha256, sha(&new.certificate));
    assert_eq!(
        verified.lineage,
        vec![sha(&old.certificate), sha(&new.certificate)]
    );
    // A lineage whose link was not signed by the key before it.
    let stranger = TestKey::new("Stranger");
    let bad = sign(
        &unsigned_apk("com.example.owned", b"dex\n035\0"),
        Signing {
            key: &new,
            v3: true,
            rotated_from: &[&stranger, &old],
            twice: false,
            claim: None,
            forge_links_with: None,
        },
    );
    // stranger -> old -> new is a valid chain of its own; break one link by
    // corrupting the old key's signature over the new node.
    assert!(verify(&bad).is_ok());
    let mut broken = bad.clone();
    let at = broken
        .windows(new.certificate.len())
        .rposition(|w| w == new.certificate.as_slice())
        .unwrap();
    // The last occurrence is the lineage node's signed data; flipping a
    // byte there breaks the link (and the signed data, which is refused).
    broken[at + 20] ^= 1;
    assert!(matches!(verify(&broken), Err(Refused::Invalid(_))));
}

/// Portable test 1716: what is not supported is said.
#[test]
fn unsupported_and_unsigned_apks_say_so() {
    let key = TestKey::new("Owner");
    let two = sign(
        &unsigned_apk("com.example.owned", b"dex\n035\0"),
        Signing {
            key: &key,
            v3: true,
            rotated_from: &[],
            twice: true,
            claim: None,
            forge_links_with: None,
        },
    );
    match verify(&two) {
        Err(Refused::Unsupported(what)) => assert!(what.contains("2 signers"), "{what}"),
        other => panic!("{other:?}"),
    }
    let unsigned = unsigned_apk("com.example.owned", b"dex\n035\0");
    assert_eq!(verify(&unsigned), Err(Refused::Unsigned));
    assert!(matches!(
        verify(b"not a zip at all"),
        Err(Refused::NotAnApk(_))
    ));
    assert!(Refused::Unsigned.to_string().contains("v1-only"));
}

fn issued(key_cert: &[u8], apk: &[u8]) -> Statement {
    let verified = verify(apk).unwrap();
    assert_eq!(verified.certificate_sha256, sha(key_cert));
    Statement::for_import(&verified, "import-1", "acct-42", "n0nce", 1_000).unwrap()
}

fn answer(statement: &Statement, key: &TestKey, rotated_from: &[&TestKey]) -> Vec<u8> {
    sign(
        &challenge_apk(statement),
        Signing {
            key,
            v3: true,
            rotated_from,
            twice: false,
            claim: None,
            forge_links_with: None,
        },
    )
}

/// Portable test 1717: the challenge binds package, certificate, the APK,
/// the import, the account, the nonce and the expiry.
#[test]
fn a_challenge_answer_proves_the_key_and_nothing_else() {
    let owner = TestKey::new("Owner");
    let app = signed(&owner, true);
    let statement = issued(&owner.certificate, &app);
    assert_eq!(statement.package, "com.example.owned");
    assert_eq!(statement.apk_sha256, sha(&app));

    let good = answer(&statement, &owner, &[]);
    assert_eq!(statement_in(&good).unwrap(), statement);
    assert!(check_answer(&good, &statement, 999).is_ok());

    // Expired.
    assert_eq!(
        check_answer(&good, &statement, 1_001),
        Err(NotProved::Expired)
    );
    // Another key.
    let thief = TestKey::new("Thief");
    assert!(matches!(
        check_answer(&answer(&statement, &thief, &[]), &statement, 1),
        Err(NotProved::WrongKey { .. })
    ));
    // Every bound field: a statement differing in any one is not this one.
    for change in [
        |s: &mut Statement| s.nonce = "other".into(),
        |s: &mut Statement| s.package = "com.example.other".into(),
        |s: &mut Statement| s.account = "acct-43".into(),
        |s: &mut Statement| s.request = "import-2".into(),
        |s: &mut Statement| s.apk_sha256 = "00".repeat(32),
        |s: &mut Statement| s.expires_at = 9_999,
    ] {
        let mut other = statement.clone();
        change(&mut other);
        let replayed = answer(&other, &owner, &[]);
        assert_eq!(
            check_answer(&replayed, &statement, 1),
            Err(NotProved::WrongStatement)
        );
    }
    // A signed answer changed afterwards.
    let mut tampered = good.clone();
    let at = tampered.windows(5).position(|w| w == b"n0nce").unwrap();
    tampered[at] = b'm';
    assert!(matches!(
        check_answer(&tampered, &statement, 1),
        Err(NotProved::Apk(_))
    ));
    // An unsigned challenge proves nothing.
    assert!(matches!(
        check_answer(&challenge_apk(&statement), &statement, 1),
        Err(NotProved::Apk(Refused::Unsigned))
    ));
}

#[test]
fn a_key_rotated_to_since_the_app_was_signed_can_answer() {
    let old = TestKey::new("Old key");
    let new = TestKey::new("New key");
    let app = signed(&old, true);
    let statement = issued(&old.certificate, &app);
    assert!(check_answer(&answer(&statement, &new, &[&old]), &statement, 1).is_ok());
    // A new key that did not rotate from the app's is not the app's.
    assert!(matches!(
        check_answer(&answer(&statement, &new, &[]), &statement, 1),
        Err(NotProved::WrongKey { .. })
    ));
}

/// Writes the unsigned inputs the committed apksigner fixtures were made
/// from (tests/fixtures/README.md says how): by hand only.
#[test]
#[ignore = "writes fixture inputs into KRATE_APKSIG_FIXTURE_DIR"]
fn write_fixture_inputs() {
    let dir = std::path::PathBuf::from(std::env::var("KRATE_APKSIG_FIXTURE_DIR").unwrap());
    std::fs::write(
        dir.join("app-unsigned.apk"),
        unsigned_apk("com.example.owned", b"dex\n035\0 the app's code"),
    )
    .unwrap();
    let statement = Statement {
        v: 1,
        purpose: challenge::PURPOSE.to_string(),
        package: "com.example.owned".into(),
        certificate_sha256: std::env::var("KRATE_APKSIG_CERT").unwrap_or_default(),
        apk_sha256: std::env::var("KRATE_APKSIG_APK").unwrap_or_default(),
        request: "fixture-import".into(),
        account: "fixture-account".into(),
        nonce: "00112233445566778899aabbccddeeff".into(),
        expires_at: 4_102_444_800,
    };
    std::fs::write(
        dir.join("challenge-unsigned.apk"),
        challenge_apk(&statement),
    )
    .unwrap();
}
