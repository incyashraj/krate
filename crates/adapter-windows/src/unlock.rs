//! Asking the person to prove it is them on Windows, for `krate:ui/unlock`
//! (IC-924, IC-982): Windows Hello -- a face, a fingerprint or the PIN -- in
//! the system's own dialog, through `UserConsentVerifier`. The app learns
//! only yes, no, or that this computer cannot check, as on a Mac and a
//! phone.
//!
//! The dialog belongs to the app's window when it has one in front
//! (`IUserConsentVerifierInterop`, the way a desktop program must ask), so it
//! rises over the app instead of somewhere behind it. The wait runs on its
//! own thread: the app's thread is the one Windows sends the window's
//! messages to, so while it waits it keeps answering them.

/// How the check went; the WIT `unlock-answer`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    Yes,
    No,
    Unavailable(String),
}

/// Why Windows Hello cannot check, from `UserConsentVerifierAvailability`
/// (0 available); None when it can.
pub fn unavailable_because(availability: i32) -> Option<&'static str> {
    match availability {
        0 => None,
        1 => Some("this computer has no Windows Hello camera, fingerprint reader or PIN"),
        2 => Some("Windows Hello is not set up for you: add a PIN or a face in Settings, Accounts, Sign-in options"),
        3 => Some("Windows Hello is turned off on this computer by its administrator"),
        4 => Some("Windows Hello is busy: try again in a moment"),
        _ => Some("Windows Hello cannot check it is you here"),
    }
}

/// What the app hears, from `UserConsentVerificationResult`.
pub fn answer_of(result: i32) -> Answer {
    match result {
        0 => Answer::Yes,
        // Cancelled, or too many tries: keep it locked.
        5 | 6 => Answer::No,
        other => Answer::Unavailable(
            unavailable_because(other)
                .unwrap_or("Windows Hello could not check it is you")
                .to_string(),
        ),
    }
}

#[cfg(windows)]
mod windows_hello {
    use super::{answer_of, unavailable_because, Answer};
    use std::sync::mpsc::{channel, RecvTimeoutError};
    use std::time::Duration;
    use windows::core::HSTRING;
    use windows::Security::Credentials::UI::{UserConsentVerificationResult, UserConsentVerifier};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::WinRT::IUserConsentVerifierInterop;
    use windows::Win32::UI::Input::KeyboardAndMouse::GetActiveWindow;
    use windows::Win32::UI::WindowsAndMessaging::{PeekMessageW, MSG, PM_NOREMOVE};
    use windows_future::IAsyncOperation;

    fn availability() -> i32 {
        UserConsentVerifier::CheckAvailabilityAsync()
            .and_then(|op| op.join())
            .map(|a| a.0)
            .unwrap_or(1)
    }

    pub fn available() -> bool {
        availability() == 0
    }

    pub fn check(reason: &str) -> Answer {
        // The app's own window, if it is the active one on this thread.
        // SAFETY: GetActiveWindow reads this thread's state and returns a
        // handle or null.
        let window = unsafe { GetActiveWindow() }.0 as isize;
        let message = HSTRING::from(reason);
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            if let Some(why) = unavailable_because(availability()) {
                let _ = tx.send(Answer::Unavailable(why.to_string()));
                return;
            }
            let asked: windows::core::Result<IAsyncOperation<UserConsentVerificationResult>> =
                if window != 0 {
                    windows::core::factory::<UserConsentVerifier, IUserConsentVerifierInterop>()
                        .and_then(|interop| {
                            // SAFETY: a window handle of this process, read on
                            // its own thread a moment ago; Windows checks it.
                            unsafe {
                                interop.RequestVerificationForWindowAsync(
                                    HWND(window as *mut core::ffi::c_void),
                                    &message,
                                )
                            }
                        })
                } else {
                    UserConsentVerifier::RequestVerificationAsync(&message)
                };
            let answer = match asked.and_then(|op| op.join()) {
                Ok(result) => answer_of(result.0),
                Err(err) => Answer::Unavailable(format!("Windows Hello would not open ({err})")),
            };
            let _ = tx.send(answer);
        });
        let deadline = std::time::Instant::now() + Duration::from_secs(300);
        loop {
            match rx.recv_timeout(Duration::from_millis(20)) {
                Ok(answer) => return answer,
                Err(RecvTimeoutError::Disconnected) => {
                    return Answer::Unavailable("Windows Hello stopped answering".to_string())
                }
                Err(RecvTimeoutError::Timeout) if std::time::Instant::now() > deadline => {
                    return Answer::No
                }
                Err(RecvTimeoutError::Timeout) => {
                    // Answer what Windows sends this thread's windows (the
                    // dialog's owner is one) without taking the posted input
                    // that is the app's.
                    let mut msg = MSG::default();
                    // SAFETY: a valid out pointer; PM_NOREMOVE leaves the
                    // queue as it was, after sent messages are delivered.
                    let _ = unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE) };
                }
            }
        }
    }
}

#[cfg(windows)]
pub use windows_hello::{available, check};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_verified_is_yes_and_a_cancel_stays_locked() {
        assert_eq!(answer_of(0), Answer::Yes);
        assert_eq!(answer_of(6), Answer::No, "cancelled");
        assert_eq!(answer_of(5), Answer::No, "too many tries");
    }

    #[test]
    fn a_computer_that_cannot_check_says_why_in_words() {
        assert_eq!(unavailable_because(0), None);
        for code in [1, 2, 3, 4, 99] {
            let why = unavailable_because(code).expect("a reason");
            assert!(why.contains("Windows Hello"), "{why}");
            assert!(matches!(answer_of(code), Answer::Unavailable(w) if w == why));
        }
        assert!(unavailable_because(2).unwrap().contains("Sign-in options"));
    }
}
