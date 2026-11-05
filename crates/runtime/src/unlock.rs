//! Asking the person to prove it is them (krate:ui/unlock, IC-924).
//!
//! On a Mac and an iPhone this is LocalAuthentication's
//! `deviceOwnerAuthentication` policy: Face ID or Touch ID, falling back to
//! the passcode or the login password, in the system's own sheet; on Android
//! BiometricPrompt; on Windows, Windows Hello. The app learns only yes, no,
//! or that the device cannot check.

/// How the check went; the WIT `unlock-answer`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Answer {
    Yes,
    No,
    Unavailable(String),
}

/// What `KRATE_UNLOCK` stands in for, headless: `yes`, `no`, anything
/// else is a device that cannot check.
pub(crate) fn stand_in(answer: &str) -> Answer {
    match answer {
        "yes" => Answer::Yes,
        "no" => Answer::No,
        _ => Answer::Unavailable("this device cannot check it is you".to_string()),
    }
}

/// Why a reason cannot be shown: the sheet names the app's purpose, so it
/// must say something, and not a page of text.
pub(crate) fn check_reason(reason: &str) -> Result<(), String> {
    let reason = reason.trim();
    if reason.is_empty() {
        return Err("say why you are asking (\"Unlock your vault\")".to_string());
    }
    if reason.chars().count() > 120 {
        return Err("the reason is longer than 120 characters".to_string());
    }
    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
mod apple {
    use super::Answer;
    use objc2::msg_send;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyClass, AnyObject, Bool};
    use objc2_foundation::NSString;

    #[link(name = "LocalAuthentication", kind = "framework")]
    unsafe extern "C" {}

    /// LAPolicyDeviceOwnerAuthentication: biometrics, then the passcode.
    const DEVICE_OWNER: isize = 2;

    fn context() -> Option<Retained<AnyObject>> {
        let class = AnyClass::get(c"LAContext")?;
        // SAFETY: +[LAContext new].
        unsafe { msg_send![class, new] }
    }

    pub(crate) fn available() -> bool {
        let Some(context) = context() else {
            return false;
        };
        // SAFETY: canEvaluatePolicy:error: with a nil error out-pointer.
        unsafe {
            let can: Bool = msg_send![
                &*context,
                canEvaluatePolicy: DEVICE_OWNER,
                error: std::ptr::null_mut::<*mut AnyObject>()
            ];
            can.as_bool()
        }
    }

    pub(crate) fn check(reason: &str) -> Answer {
        let Some(context) = context() else {
            return Answer::Unavailable("this system has no LocalAuthentication".to_string());
        };
        // SAFETY: canEvaluatePolicy:error: reporting why not into `error`.
        unsafe {
            let mut error: *mut AnyObject = std::ptr::null_mut();
            let can: Bool =
                msg_send![&*context, canEvaluatePolicy: DEVICE_OWNER, error: &mut error];
            if !can.as_bool() {
                return Answer::Unavailable(
                    describe(error)
                        .unwrap_or_else(|| "this device cannot check it is you".to_string()),
                );
            }
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let tx = std::sync::Mutex::new(Some(tx));
        let reply = block2::RcBlock::new(move |ok: Bool, error: *mut AnyObject| {
            let answer = if ok.as_bool() {
                Answer::Yes
            } else {
                // SAFETY: the NSError LocalAuthentication handed the block.
                let code: isize = unsafe { error.as_ref().map_or(0, |e| msg_send![e, code]) };
                match code {
                    // Failed, cancelled by the person, chose the fallback,
                    // cancelled by the system or the app: keep it locked.
                    -1 | -2 | -3 | -4 | -9 => Answer::No,
                    _ => Answer::Unavailable(
                        describe(error).unwrap_or_else(|| "the check could not run".to_string()),
                    ),
                }
            };
            if let Some(tx) = tx.lock().expect("unlock lock").take() {
                let _ = tx.send(answer);
            }
        });
        // SAFETY: evaluatePolicy:localizedReason:reply: on a live context;
        // the reply block runs once on LocalAuthentication's own queue.
        unsafe {
            let _: () = msg_send![
                &*context,
                evaluatePolicy: DEVICE_OWNER,
                localizedReason: &*NSString::from_str(reason),
                reply: &*reply
            ];
        }
        rx.recv_timeout(std::time::Duration::from_secs(300))
            .unwrap_or(Answer::No)
    }

    fn describe(error: *mut AnyObject) -> Option<String> {
        // SAFETY: an NSError's localizedDescription, when there is one.
        unsafe {
            let error = error.as_ref()?;
            let text: Retained<NSString> = msg_send![error, localizedDescription];
            Some(text.to_string())
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
pub(crate) use apple::{available, check};

/// Android: the system's BiometricPrompt, fingerprint or face with the
/// screen lock as the other way in (IC-924).
#[cfg(target_os = "android")]
pub(crate) fn available() -> bool {
    krate_adapter_android::unlock::available()
}

#[cfg(target_os = "android")]
pub(crate) fn check(reason: &str) -> Answer {
    match krate_adapter_android::unlock::check(reason) {
        krate_adapter_android::unlock::Answer::Yes => Answer::Yes,
        krate_adapter_android::unlock::Answer::No => Answer::No,
        krate_adapter_android::unlock::Answer::Unavailable(why) => Answer::Unavailable(why),
    }
}

/// Windows: Windows Hello -- face, fingerprint or PIN -- in the system's
/// own dialog over the app's window (IC-982).
#[cfg(target_os = "windows")]
pub(crate) fn available() -> bool {
    krate_adapter_windows::unlock::available()
}

#[cfg(target_os = "windows")]
pub(crate) fn check(reason: &str) -> Answer {
    match krate_adapter_windows::unlock::check(reason) {
        krate_adapter_windows::unlock::Answer::Yes => Answer::Yes,
        krate_adapter_windows::unlock::Answer::No => Answer::No,
        krate_adapter_windows::unlock::Answer::Unavailable(why) => Answer::Unavailable(why),
    }
}

/// Linux has no one system check every desktop shares (polkit asks for the
/// account password to act as administrator, not to say it is the person
/// in front of an app), so it answers unavailable, and says so.
#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "android",
    target_os = "windows"
)))]
pub(crate) fn available() -> bool {
    false
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "android",
    target_os = "windows"
)))]
pub(crate) fn check(_reason: &str) -> Answer {
    Answer::Unavailable(
        "this computer has no system check that it is you: use the app's own password".to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stand_in_answers_as_the_person_would() {
        assert_eq!(stand_in("yes"), Answer::Yes);
        assert_eq!(stand_in("no"), Answer::No);
        assert!(matches!(stand_in("none"), Answer::Unavailable(_)));
    }

    #[test]
    fn a_reason_must_say_something_short() {
        assert!(check_reason("Unlock your vault").is_ok());
        assert!(check_reason("   ").is_err());
        assert!(check_reason(&"x".repeat(121)).is_err());
    }
}
