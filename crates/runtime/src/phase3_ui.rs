//! Phase 3 UI dispatcher scaffold.
//!
//! This module is the first runtime-facing Phase 3 UI boundary. It still uses
//! the shared in-memory draft registry from `adapter-common`; native AppKit,
//! Win32, and GTK windows come later.

use layer36_adapter_common::ui::{
    DraftWindowRegistry, UiAdapterError, UiEvent, WindowId, WindowOptions, WindowRecord, WindowSize,
};
use thiserror::Error;

use crate::uapi::{UapiCall, UapiError, UapiGuard, UiCall};

pub type UiDispatchResult<T> = std::result::Result<T, UiDispatchError>;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum UiDispatchError {
    #[error("permission denied")]
    PermissionDenied,
    #[error("UI adapter error: {0}")]
    Adapter(#[from] UiAdapterError),
    #[error("policy error: {0}")]
    Policy(String),
    #[error("operation is not implemented in the Phase 3 draft UI dispatcher yet")]
    Unsupported,
}

pub struct Phase3UiDispatcher<'a> {
    guard: &'a UapiGuard,
    windows: &'a mut DraftWindowRegistry,
}

impl<'a> Phase3UiDispatcher<'a> {
    pub fn new(guard: &'a UapiGuard, windows: &'a mut DraftWindowRegistry) -> Self {
        Self { guard, windows }
    }

    pub fn create_window(&mut self, options: WindowOptions) -> UiDispatchResult<WindowId> {
        self.check_window_access()?;
        Ok(self.windows.create_window(options))
    }

    pub fn show_window(&mut self, id: WindowId) -> UiDispatchResult<()> {
        self.check_window_access()?;
        self.windows.show_window(id)?;
        Ok(())
    }

    pub fn close_window(&mut self, id: WindowId) -> UiDispatchResult<()> {
        self.check_window_access()?;
        self.windows.close_window(id)?;
        Ok(())
    }

    pub fn set_title(&mut self, id: WindowId, title: impl Into<String>) -> UiDispatchResult<()> {
        self.check_window_access()?;
        self.windows.set_title(id, title)?;
        Ok(())
    }

    pub fn set_size(&mut self, id: WindowId, size: WindowSize) -> UiDispatchResult<()> {
        self.check_window_access()?;
        self.windows.set_size(id, size)?;
        Ok(())
    }

    pub fn request_redraw(&mut self, id: WindowId) -> UiDispatchResult<()> {
        self.check_window_access()?;
        self.windows.request_redraw(id)?;
        Ok(())
    }

    pub fn read_clipboard_text(&self) -> UiDispatchResult<String> {
        self.check(&UapiCall::Ui(UiCall::ClipboardRead))?;
        Err(UiDispatchError::Unsupported)
    }

    pub fn write_clipboard_text(&self, _text: &str) -> UiDispatchResult<()> {
        self.check(&UapiCall::Ui(UiCall::ClipboardWrite))?;
        Err(UiDispatchError::Unsupported)
    }

    pub fn window(&self, id: WindowId) -> Option<&WindowRecord> {
        self.windows.window(id)
    }

    pub fn drain_events(&mut self) -> Vec<UiEvent> {
        self.windows.drain_events()
    }

    fn check_window_access(&self) -> UiDispatchResult<()> {
        self.check(&UapiCall::Ui(UiCall::WindowCreate))
    }

    fn check(&self, call: &UapiCall) -> UiDispatchResult<()> {
        self.guard.check(call).map(|_| ()).map_err(map_ui_policy)
    }
}

fn map_ui_policy(err: UapiError) -> UiDispatchError {
    if matches!(
        err,
        UapiError::Policy(layer36_policy::PolicyError::Denied { .. })
    ) {
        UiDispatchError::PermissionDenied
    } else {
        UiDispatchError::Policy(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use layer36_adapter_common::ui::{DraftWindowRegistry, UiEvent, WindowOptions, WindowSize};
    use layer36_policy::SessionPolicy;

    use super::*;

    #[test]
    fn default_window_grant_creates_and_tracks_draft_window() {
        let guard = UapiGuard::new(SessionPolicy::default());
        let mut registry = DraftWindowRegistry::default();
        let size = WindowSize::new(800, 600).expect("size");
        let mut dispatcher = Phase3UiDispatcher::new(&guard, &mut registry);

        let id = dispatcher
            .create_window(WindowOptions::new("Layer36 Notes", size).expect("options"))
            .expect("create window");
        dispatcher.show_window(id).expect("show window");
        let resized = WindowSize::new(1024, 768).expect("resized");
        dispatcher.set_size(id, resized).expect("resize window");
        dispatcher.request_redraw(id).expect("redraw");
        dispatcher.close_window(id).expect("close window");

        let window = dispatcher.window(id).expect("window");
        assert_eq!(window.title, "Layer36 Notes");
        assert_eq!(window.size, resized);
        assert!(!window.visible);
        assert!(window.closed);
        assert_eq!(
            dispatcher.drain_events(),
            vec![
                UiEvent::WindowCreated(id),
                UiEvent::WindowShown(id),
                UiEvent::Resized { id, size: resized },
                UiEvent::RedrawRequested(id),
                UiEvent::WindowClosed(id),
            ]
        );
    }

    #[test]
    fn window_operations_reuse_adapter_validation() {
        let guard = UapiGuard::new(SessionPolicy::default());
        let mut registry = DraftWindowRegistry::default();
        let size = WindowSize::new(640, 480).expect("size");
        let mut dispatcher = Phase3UiDispatcher::new(&guard, &mut registry);

        let id = dispatcher
            .create_window(WindowOptions::new("Notes", size).expect("options"))
            .expect("create window");
        let err = dispatcher
            .set_title(id, " ")
            .expect_err("empty title should fail");

        assert!(matches!(
            err,
            UiDispatchError::Adapter(UiAdapterError::EmptyTitle)
        ));
    }

    #[test]
    fn clipboard_read_denies_before_unsupported_draft_path() {
        let guard = UapiGuard::new(SessionPolicy::default());
        let mut registry = DraftWindowRegistry::default();
        let dispatcher = Phase3UiDispatcher::new(&guard, &mut registry);

        let err = dispatcher
            .read_clipboard_text()
            .expect_err("clipboard should need explicit grant");

        assert!(matches!(err, UiDispatchError::PermissionDenied));
    }

    #[test]
    fn clipboard_read_reaches_draft_unsupported_when_granted() {
        let policy =
            SessionPolicy::from_cli_grants(&["ui.clipboard:read".to_string()]).expect("policy");
        let guard = UapiGuard::new(policy);
        let mut registry = DraftWindowRegistry::default();
        let dispatcher = Phase3UiDispatcher::new(&guard, &mut registry);

        let err = dispatcher
            .read_clipboard_text()
            .expect_err("clipboard host adapter is not implemented yet");

        assert!(matches!(err, UiDispatchError::Unsupported));
    }
}
