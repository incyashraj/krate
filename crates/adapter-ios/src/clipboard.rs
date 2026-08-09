//! Clipboard stubs for iOS.
//!
//! There is no arboard backend on iOS; the platform clipboard needs UIPasteboard,
//! which is M3 work in the mobile plan. Until then both
//! directions answer Unsupported, the same honest degradation macOS reports
//! from the drawn path today -- apps already handle it.

use krate_adapter_common::ui::UiAdapterError;

pub(crate) fn read_text() -> Result<String, UiAdapterError> {
    Err(UiAdapterError::Unsupported(
        "clipboard is not wired on iOS yet".to_string(),
    ))
}

pub(crate) fn write_text(_text: &str) -> Result<(), UiAdapterError> {
    Err(UiAdapterError::Unsupported(
        "clipboard is not wired on iOS yet".to_string(),
    ))
}
