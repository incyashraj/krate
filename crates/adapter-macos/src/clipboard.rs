//! macOS system pasteboard access for the drawn path's Cut/Copy/Paste.
//!
//! The drawn host paints its own text, so the guest can only reach the system
//! pasteboard through the host. arboard is cross-platform and drives
//! `NSPasteboard` on macOS, so this mirrors `adapter-linux`'s clipboard module:
//! open a handle per call rather than caching one, keeping the surface a plain
//! read/write with no background state to reason about.
//!
//! Off macOS this crate has no native pasteboard, so these report the feature
//! as unsupported.

use krate_adapter_common::ui::UiAdapterError;

/// Read UTF-8 text from the system pasteboard.
#[cfg(target_os = "macos")]
pub(crate) fn read_text() -> Result<String, UiAdapterError> {
    let mut clipboard =
        arboard::Clipboard::new().map_err(|err| UiAdapterError::Internal(err.to_string()))?;
    match clipboard.get_text() {
        Ok(text) => Ok(text),
        // An empty or non-text pasteboard is not an error: pasting nothing
        // should leave the note untouched, not fail the whole edit.
        Err(arboard::Error::ContentNotAvailable) => Ok(String::new()),
        Err(err) => Err(UiAdapterError::Internal(err.to_string())),
    }
}

/// Write UTF-8 text to the system pasteboard.
#[cfg(target_os = "macos")]
pub(crate) fn write_text(text: &str) -> Result<(), UiAdapterError> {
    let mut clipboard =
        arboard::Clipboard::new().map_err(|err| UiAdapterError::Internal(err.to_string()))?;
    clipboard
        .set_text(text.to_string())
        .map_err(|err| UiAdapterError::Internal(err.to_string()))
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn read_text() -> Result<String, UiAdapterError> {
    Err(UiAdapterError::Unsupported(
        "clipboard is only wired on the native macOS backend".to_string(),
    ))
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn write_text(_text: &str) -> Result<(), UiAdapterError> {
    Err(UiAdapterError::Unsupported(
        "clipboard is only wired on the native macOS backend".to_string(),
    ))
}
