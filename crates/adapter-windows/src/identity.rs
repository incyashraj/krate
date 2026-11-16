//! Who a running app is to Windows (IC-278, per-artifact identity).
//!
//! Every Krate app runs as `krate.exe`, so without this Windows grouped every
//! one of them under one taskbar button named after the engine, and a
//! pinned app reopened Krate rather than itself. A process that names its
//! own AppUserModelID before it opens a window gets its own taskbar group;
//! the id's `DisplayName` (the same key its toasts use) is the name shown.
//!
//! The id comes from the app's lineage, never its display name, so two apps
//! called "Notes" from two publishers are two apps here too.

/// Make this process the app known by `identity`, shown as `app_name`.
///
/// Called once, before the first window. Best effort: if Windows refuses,
/// the app still runs, grouped under Krate as it was before.
#[cfg(windows)]
pub fn adopt(identity: &str, app_name: &str) -> Result<String, String> {
    use windows::core::HSTRING;
    use windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;

    let id = crate::toast::app_user_model_id(identity);
    crate::toast::register_sender(&id, app_name)?;
    // SAFETY: the HSTRING outlives the call, which copies it.
    unsafe { SetCurrentProcessExplicitAppUserModelID(&HSTRING::from(&id)) }
        .map_err(|err| format!("Windows would not take the app's identity ({err})"))?;
    Ok(id)
}

#[cfg(all(test, windows))]
mod tests {
    /// On a real Windows desktop (CI's windows-2022 runner): the process
    /// reads back the id it adopted, the lineage's and not the engine's.
    #[test]
    fn the_process_takes_the_apps_own_id() {
        use windows::Win32::System::Com::CoTaskMemFree;
        use windows::Win32::UI::Shell::GetCurrentProcessExplicitAppUserModelID;

        let id = super::adopt("dev.krate.identity-test@0123456789abcdef", "Identity test")
            .expect("adopt");
        // SAFETY: on success the string is ours to free, and is freed once.
        let held = unsafe {
            let raw = GetCurrentProcessExplicitAppUserModelID().expect("read back");
            let text = raw.to_string().expect("utf-16");
            CoTaskMemFree(Some(raw.0 as *const _));
            text
        };
        assert_eq!(held, id);
        assert!(id.starts_with("Krate.App."), "{id}");
    }
}
