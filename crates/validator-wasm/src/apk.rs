//! The Android signing-key challenge at the hub's door (IC-800): the same
//! verifier `krate port` runs, so the hub never judges an APK signature
//! with a JavaScript reading of the format.

use krate_apksig::challenge::{check_answer, statement_in, NotProved, Statement};
use serde_json::json;

/// The statement a challenge answer carries, unchecked -- what the hub
/// looks its record up by. `{"ok":true,"statement":{...}}` or a problem.
pub fn apk_statement_to_json(apk: &[u8]) -> String {
    match statement_in(apk) {
        Ok(statement) => json!({ "ok": true, "statement": statement }).to_string(),
        Err(err) => json!({ "ok": false, "problem": err.to_string() }).to_string(),
    }
}

/// Check a signed answer: the frame is a little-endian u32 length, that
/// many bytes of `{"issued": Statement, "now": seconds}`, then the APK.
/// `{"ok":true,"certificate_sha256":..,"lineage":[..]}` or
/// `{"ok":false,"reason":..,"problem":..}`.
pub fn apk_check_to_json(frame: &[u8]) -> String {
    let refused = |reason: &str, problem: String| {
        json!({ "ok": false, "reason": reason, "problem": problem }).to_string()
    };
    let Some((head, rest)) = frame.split_at_checked(4) else {
        return refused("frame", "the request was cut short".into());
    };
    let len = u32::from_le_bytes(head.try_into().expect("four bytes")) as usize;
    let Some((meta, apk)) = rest.split_at_checked(len) else {
        return refused("frame", "the request was cut short".into());
    };
    let ask: serde_json::Value = match serde_json::from_slice(meta) {
        Ok(ask) => ask,
        Err(err) => return refused("frame", format!("the request did not read: {err}")),
    };
    let issued: Statement = match serde_json::from_value(ask["issued"].clone()) {
        Ok(issued) => issued,
        Err(err) => return refused("frame", format!("the issued statement did not read: {err}")),
    };
    let Some(now) = ask["now"].as_u64() else {
        return refused("frame", "the request names no time".into());
    };
    match check_answer(apk, &issued, now) {
        Ok(verified) => json!({
            "ok": true,
            "certificate_sha256": verified.certificate_sha256,
            "lineage": verified.lineage,
            "scheme": verified.scheme,
        })
        .to_string(),
        Err(err) => {
            let reason = match &err {
                NotProved::Apk(_) => "apk",
                NotProved::WrongStatement => "wrong-statement",
                NotProved::Expired => "expired",
                NotProved::WrongKey { .. } => "wrong-key",
            };
            refused(reason, err.to_string())
        }
    }
}
