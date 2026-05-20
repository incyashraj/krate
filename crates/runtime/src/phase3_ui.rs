//! Phase 3 UI dispatcher scaffold.
//!
//! This module is the first runtime-facing Phase 3 UI boundary. It still uses
//! the shared UI adapter trait from `adapter-common`; native AppKit, Win32, and
//! GTK windows come later.

use layer36_adapter_common::ui::{
    UiAdapter, UiAdapterError, UiAdapterInfo, UiEvent, WindowId, WindowOptions, WindowRecord,
    WindowSize,
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
    adapter: &'a dyn UiAdapter,
}

/// Owns the Phase 3 UI guard and host adapter for one runtime session.
pub struct Phase3UiRuntime {
    guard: UapiGuard,
    adapter: Box<dyn UiAdapter>,
}

impl Phase3UiRuntime {
    /// Build a UI runtime with an explicit adapter, mainly for tests.
    pub fn new(guard: UapiGuard, adapter: Box<dyn UiAdapter>) -> Self {
        Self { guard, adapter }
    }

    /// Build a UI runtime using the current host adapter entry point.
    pub fn with_host_adapter(guard: UapiGuard) -> Self {
        Self::new(guard, discover_host_ui_adapter())
    }

    /// Return a dispatcher that checks policy before each adapter call.
    pub fn dispatcher(&self) -> Phase3UiDispatcher<'_> {
        Phase3UiDispatcher::new(&self.guard, self.adapter.as_ref())
    }

    /// Return the selected adapter information.
    pub fn adapter_info(&self) -> UiAdapterInfo {
        self.adapter.info()
    }
}

impl<'a> Phase3UiDispatcher<'a> {
    pub fn new(guard: &'a UapiGuard, adapter: &'a dyn UiAdapter) -> Self {
        Self { guard, adapter }
    }

    pub fn adapter_info(&self) -> UiAdapterInfo {
        self.adapter.info()
    }

    pub fn create_window(&self, options: WindowOptions) -> UiDispatchResult<WindowId> {
        self.check_window_access()?;
        Ok(self.adapter.create_window(options)?)
    }

    pub fn show_window(&self, id: WindowId) -> UiDispatchResult<()> {
        self.check_window_access()?;
        self.adapter.show_window(id)?;
        Ok(())
    }

    pub fn close_window(&self, id: WindowId) -> UiDispatchResult<()> {
        self.check_window_access()?;
        self.adapter.close_window(id)?;
        Ok(())
    }

    pub fn set_title(&self, id: WindowId, title: impl Into<String>) -> UiDispatchResult<()> {
        self.check_window_access()?;
        self.adapter.set_title(id, title.into())?;
        Ok(())
    }

    pub fn set_size(&self, id: WindowId, size: WindowSize) -> UiDispatchResult<()> {
        self.check_window_access()?;
        self.adapter.set_size(id, size)?;
        Ok(())
    }

    pub fn request_redraw(&self, id: WindowId) -> UiDispatchResult<()> {
        self.check_window_access()?;
        self.adapter.request_redraw(id)?;
        Ok(())
    }

    pub fn read_clipboard_text(&self) -> UiDispatchResult<String> {
        self.check(&UapiCall::Ui(UiCall::ClipboardRead))?;
        self.adapter.read_clipboard_text().map_err(Into::into)
    }

    pub fn write_clipboard_text(&self, text: &str) -> UiDispatchResult<()> {
        self.check(&UapiCall::Ui(UiCall::ClipboardWrite))?;
        self.adapter.write_clipboard_text(text).map_err(Into::into)
    }

    pub fn window(&self, id: WindowId) -> UiDispatchResult<Option<WindowRecord>> {
        Ok(self.adapter.window(id)?)
    }

    pub fn drain_events(&self) -> UiDispatchResult<Vec<UiEvent>> {
        Ok(self.adapter.drain_events()?)
    }

    fn check_window_access(&self) -> UiDispatchResult<()> {
        self.check(&UapiCall::Ui(UiCall::WindowCreate))
    }

    fn check(&self, call: &UapiCall) -> UiDispatchResult<()> {
        self.guard.check(call).map(|_| ()).map_err(map_ui_policy)
    }
}

fn discover_host_ui_adapter() -> Box<dyn UiAdapter> {
    #[cfg(target_os = "linux")]
    {
        Box::new(layer36_adapter_linux::discover_ui_adapter())
    }

    #[cfg(target_os = "macos")]
    {
        Box::new(layer36_adapter_macos::discover_ui_adapter())
    }

    #[cfg(target_os = "windows")]
    {
        Box::new(layer36_adapter_windows::discover_ui_adapter())
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        Box::new(layer36_adapter_common::ui::DraftUiAdapter::new())
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
    use layer36_adapter_common::ui::{DraftUiAdapter, UiEvent, WindowOptions, WindowSize};
    use layer36_policy::SessionPolicy;

    use super::*;

    #[test]
    fn default_window_grant_creates_and_tracks_draft_window() {
        let guard = UapiGuard::new(SessionPolicy::default());
        let adapter = DraftUiAdapter::default();
        let size = WindowSize::new(800, 600).expect("size");
        let dispatcher = Phase3UiDispatcher::new(&guard, &adapter);

        let id = dispatcher
            .create_window(WindowOptions::new("Layer36 Notes", size).expect("options"))
            .expect("create window");
        dispatcher.show_window(id).expect("show window");
        let resized = WindowSize::new(1024, 768).expect("resized");
        dispatcher.set_size(id, resized).expect("resize window");
        dispatcher.request_redraw(id).expect("redraw");
        dispatcher.close_window(id).expect("close window");

        let window = dispatcher.window(id).expect("adapter").expect("window");
        assert_eq!(window.title, "Layer36 Notes");
        assert_eq!(window.size, resized);
        assert!(!window.visible);
        assert!(window.closed);
        assert_eq!(
            dispatcher.drain_events().expect("events"),
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
    fn runtime_discovers_current_host_ui_adapter() {
        let guard = UapiGuard::new(SessionPolicy::default());
        let runtime = Phase3UiRuntime::with_host_adapter(guard);
        let info = runtime.adapter_info();
        let dispatcher = runtime.dispatcher();
        let size = WindowSize::new(480, 320).expect("size");

        assert!(info.backend.ends_with("headless-draft"));
        assert!(!info.native_windows);
        assert!(!info.native_event_loop);

        let id = dispatcher
            .create_window(WindowOptions::new("Layer36 host adapter", size).expect("options"))
            .expect("create window through discovered adapter");
        dispatcher.show_window(id).expect("show");

        let window = dispatcher.window(id).expect("adapter").expect("window");
        assert_eq!(window.title, "Layer36 host adapter");
        assert_eq!(
            dispatcher.drain_events().expect("events"),
            vec![UiEvent::WindowCreated(id), UiEvent::WindowShown(id)]
        );
    }

    #[test]
    fn window_operations_reuse_adapter_validation() {
        let guard = UapiGuard::new(SessionPolicy::default());
        let adapter = DraftUiAdapter::default();
        let size = WindowSize::new(640, 480).expect("size");
        let dispatcher = Phase3UiDispatcher::new(&guard, &adapter);

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
        let adapter = DraftUiAdapter::default();
        let dispatcher = Phase3UiDispatcher::new(&guard, &adapter);

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
        let adapter = DraftUiAdapter::default();
        let dispatcher = Phase3UiDispatcher::new(&guard, &adapter);

        let err = dispatcher
            .read_clipboard_text()
            .expect_err("clipboard host adapter is not implemented yet");

        assert!(matches!(
            err,
            UiDispatchError::Adapter(UiAdapterError::Unsupported(_))
        ));
    }
}
