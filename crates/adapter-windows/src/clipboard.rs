//! OS clipboard access for the drawn path's Cut/Copy/Paste.
//!
//! The drawn host paints its own text, so the guest can only reach the system
//! clipboard through the host. arboard is opened per call rather than cached,
//! keeping the surface a plain read/write with no background state. Off Windows
//! this crate has no native window backend, so these mirror `winit_native`'s
//! stubs and report the feature as unsupported.

use krate_adapter_common::ui::UiAdapterError;

/// Read UTF-8 text from the system clipboard.
#[cfg(target_os = "windows")]
pub(crate) fn read_text() -> Result<String, UiAdapterError> {
    let mut clipboard =
        arboard::Clipboard::new().map_err(|err| UiAdapterError::Internal(err.to_string()))?;
    match clipboard.get_text() {
        Ok(text) => Ok(text),
        // An empty or non-text clipboard is not an error: pasting nothing
        // should leave the note untouched, not fail the whole edit.
        Err(arboard::Error::ContentNotAvailable) => Ok(String::new()),
        Err(err) => Err(UiAdapterError::Internal(err.to_string())),
    }
}

/// Write UTF-8 text to the system clipboard.
#[cfg(target_os = "windows")]
pub(crate) fn write_text(text: &str) -> Result<(), UiAdapterError> {
    let mut clipboard =
        arboard::Clipboard::new().map_err(|err| UiAdapterError::Internal(err.to_string()))?;
    clipboard
        .set_text(text.to_string())
        .map_err(|err| UiAdapterError::Internal(err.to_string()))
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn read_text() -> Result<String, UiAdapterError> {
    Err(UiAdapterError::Unsupported(
        "clipboard is only wired on the native Windows backend".to_string(),
    ))
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn write_text(_text: &str) -> Result<(), UiAdapterError> {
    Err(UiAdapterError::Unsupported(
        "clipboard is only wired on the native Windows backend".to_string(),
    ))
}
