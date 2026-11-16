//! The signing-key challenge (IC-800): proof that the person importing an
//! APK holds the key it is signed with, today, for this import.
//!
//! The hub issues a [`Statement`] -- package, certificate, the exact APK's
//! digest, the import request, the account, a one-time nonce and an expiry
//! -- and keeps it under its nonce. Krate puts it in a small unsigned APK
//! ([`challenge_apk`]); the person signs that with the same tools and key
//! they sign their app with; [`check_answer`] verifies the signed APK with
//! the same verifier as any APK and checks it carries exactly the statement
//! issued, signed by that certificate (or by a key it rotated to). The
//! hub's own record makes the nonce single-use and binds the account; this
//! crate checks everything that is in the bytes.

use serde::{Deserialize, Serialize};

use crate::{verify, Refused, Verified};

/// Where the statement sits inside the challenge APK.
pub const STATEMENT_ENTRY: &str = "krate-key-challenge.json";

/// What the person signs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Statement {
    /// Format version: 1.
    pub v: u32,
    /// In words, so a person reading the file knows what signing it does.
    pub purpose: String,
    pub package: String,
    /// The certificate the APK being imported is signed with (its newest
    /// signer), as `apksigner verify --print-certs` prints its SHA-256.
    pub certificate_sha256: String,
    /// The exact APK being imported.
    pub apk_sha256: String,
    /// The import this proof is for.
    pub request: String,
    /// The account asking.
    pub account: String,
    pub nonce: String,
    /// Unix seconds; the answer must arrive before.
    pub expires_at: u64,
}

/// The words every statement carries.
pub const PURPOSE: &str = "Signing this proves to Krate (krate.tech) that you hold this app's \
                           signing key, for one import. It cannot be installed and grants \
                           nothing else.";

impl Statement {
    /// The statement for one import of a verified APK.
    pub fn for_import(
        verified: &Verified,
        request: &str,
        account: &str,
        nonce: &str,
        expires_at: u64,
    ) -> Result<Statement, Refused> {
        let package = verified.package.clone().ok_or_else(|| {
            Refused::NotAnApk("it has no AndroidManifest.xml naming its package".into())
        })?;
        Ok(Statement {
            v: 1,
            purpose: PURPOSE.to_string(),
            package,
            certificate_sha256: verified.certificate_sha256.clone(),
            apk_sha256: verified.apk_sha256.clone(),
            request: request.to_string(),
            account: account.to_string(),
            nonce: nonce.to_string(),
            expires_at,
        })
    }
}

/// The unsigned APK the person signs: the statement and nothing else.
pub fn challenge_apk(statement: &Statement) -> Vec<u8> {
    let json = serde_json::to_vec_pretty(statement).expect("a statement serialises");
    crate::zip::write_stored(&[(STATEMENT_ENTRY, &json)])
}

/// The statement a (signed or not) challenge APK carries, unchecked: what
/// the hub looks its record up by, before checking anything.
pub fn statement_in(apk: &[u8]) -> Result<Statement, Refused> {
    let bytes = crate::read_entry(apk, STATEMENT_ENTRY)?
        .ok_or_else(|| Refused::NotAnApk(format!("it has no {STATEMENT_ENTRY}")))?;
    serde_json::from_slice(&bytes)
        .map_err(|err| Refused::NotAnApk(format!("its {STATEMENT_ENTRY} does not read: {err}")))
}

/// Why an answer did not prove control.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotProved {
    /// The signed APK itself did not verify.
    Apk(Refused),
    /// It carries a statement other than the one issued.
    WrongStatement,
    /// Answered after the statement's expiry.
    Expired,
    /// Signed with a key that is not the app's.
    WrongKey {
        signed_with: String,
        expected: String,
    },
}

impl std::fmt::Display for NotProved {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NotProved::Apk(refused) => write!(f, "{refused}"),
            NotProved::WrongStatement => {
                write!(
                    f,
                    "the signed file is not the challenge issued for this import"
                )
            }
            NotProved::Expired => write!(f, "the challenge expired; ask for a new one"),
            NotProved::WrongKey {
                signed_with,
                expected,
            } => write!(
                f,
                "it was signed with certificate {signed_with}, and the app is signed with \
                 {expected}: sign the challenge with the app's own key"
            ),
        }
    }
}

/// Check a signed answer against the statement that was issued.
pub fn check_answer(answer: &[u8], issued: &Statement, now: u64) -> Result<Verified, NotProved> {
    let verified = verify(answer).map_err(NotProved::Apk)?;
    let carried = statement_in(answer).map_err(NotProved::Apk)?;
    if &carried != issued {
        return Err(NotProved::WrongStatement);
    }
    if now > issued.expires_at {
        return Err(NotProved::Expired);
    }
    // The app's own key, or a key that key rotated to since the APK was
    // signed (its lineage names the app's certificate).
    let same = verified.certificate_sha256 == issued.certificate_sha256;
    let rotated_from = verified.lineage.contains(&issued.certificate_sha256);
    if !same && !rotated_from {
        return Err(NotProved::WrongKey {
            signed_with: verified.certificate_sha256.clone(),
            expected: issued.certificate_sha256.clone(),
        });
    }
    Ok(verified)
}
