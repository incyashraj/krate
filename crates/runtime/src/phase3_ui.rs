//! Phase 3 UI dispatcher scaffold.
//!
//! This module is the first runtime-facing Phase 3 UI boundary. It still uses
//! the shared UI adapter trait from `adapter-common`; native AppKit, Win32, and
//! GTK windows come later.

use layer36_adapter_common::ui::{
    UiAdapter, UiAdapterError, UiAdapterInfo, UiEvent, WidgetId, WidgetNode, WidgetTree, WindowId,
    WindowOptions, WindowRecord, WindowSize,
};
use layer36_layout::{compute_layout, LayoutSnapshot, LayoutViewport};
use thiserror::Error;

use crate::uapi::{UapiCall, UapiError, UapiGuard, UiCall};

pub type UiDispatchResult<T> = std::result::Result<T, UiDispatchError>;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum UiDispatchError {
    #[error("permission denied")]
    PermissionDenied,
    #[error("UI adapter error: {0}")]
    Adapter(#[from] UiAdapterError),
    #[error("layout error: {0}")]
    Layout(String),
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

    pub fn set_root(&self, window: WindowId, root: WidgetNode) -> UiDispatchResult<()> {
        self.check_window_access()?;
        self.adapter.set_root(window, root)?;
        Ok(())
    }

    pub fn upsert_node(&self, window: WindowId, node: WidgetNode) -> UiDispatchResult<()> {
        self.check_window_access()?;
        self.adapter.upsert_node(window, node)?;
        Ok(())
    }

    pub fn remove_node(&self, window: WindowId, widget: WidgetId) -> UiDispatchResult<()> {
        self.check_window_access()?;
        self.adapter.remove_node(window, widget)?;
        Ok(())
    }

    pub fn focus_node(&self, window: WindowId, widget: WidgetId) -> UiDispatchResult<()> {
        self.check_window_access()?;
        self.adapter.focus_node(window, widget)?;
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

    pub fn widget_tree(&self, window: WindowId) -> UiDispatchResult<Option<WidgetTree>> {
        Ok(self.adapter.widget_tree(window)?)
    }

    pub fn focused_widget(&self, window: WindowId) -> UiDispatchResult<Option<WidgetId>> {
        Ok(self.adapter.focused_widget(window)?)
    }

    pub fn compute_layout(
        &self,
        window: WindowId,
        viewport: LayoutViewport,
    ) -> UiDispatchResult<LayoutSnapshot> {
        self.check_window_access()?;
        let tree = self
            .adapter
            .widget_tree(window)?
            .ok_or(UiAdapterError::MissingWidgetTree {
                window: window.get(),
            })?;

        compute_layout(&tree, viewport).map_err(|err| UiDispatchError::Layout(err.to_string()))
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
    use layer36_adapter_common::ui::{
        DraftUiAdapter, UiEvent, WidgetId, WidgetKind, WidgetNode, WidgetStyle, WindowOptions,
        WindowSize,
    };
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
    fn widget_tree_operations_pass_through_window_policy() {
        let guard = UapiGuard::new(SessionPolicy::default());
        let adapter = DraftUiAdapter::default();
        let size = WindowSize::new(640, 480).expect("size");
        let dispatcher = Phase3UiDispatcher::new(&guard, &adapter);
        let window = dispatcher
            .create_window(WindowOptions::new("Notes", size).expect("options"))
            .expect("create window");
        let root = WidgetNode::new(WidgetId::new(1).expect("root"), WidgetKind::Stack);
        let button = WidgetNode::new(WidgetId::new(2).expect("button"), WidgetKind::Button)
            .with_parent(root.id)
            .with_label("Save")
            .expect("label");

        dispatcher.set_root(window, root).expect("set root");
        dispatcher
            .upsert_node(window, button)
            .expect("upsert button");
        dispatcher
            .focus_node(window, WidgetId::new(2).expect("button"))
            .expect("focus button");

        let tree = dispatcher
            .widget_tree(window)
            .expect("tree lookup")
            .expect("tree");
        assert_eq!(tree.nodes().len(), 2);
        assert_eq!(
            dispatcher.focused_widget(window).expect("focus"),
            Some(WidgetId::new(2).expect("button"))
        );
        assert_eq!(
            dispatcher.drain_events().expect("events"),
            vec![
                UiEvent::WindowCreated(window),
                UiEvent::WidgetRootSet {
                    window,
                    root: WidgetId::new(1).expect("root"),
                },
                UiEvent::WidgetUpdated {
                    window,
                    widget: WidgetId::new(2).expect("button"),
                },
                UiEvent::FocusChanged {
                    window,
                    widget: WidgetId::new(2).expect("button"),
                },
            ]
        );
    }

    #[test]
    fn widget_tree_operations_reuse_adapter_validation() {
        let guard = UapiGuard::new(SessionPolicy::default());
        let adapter = DraftUiAdapter::default();
        let size = WindowSize::new(640, 480).expect("size");
        let dispatcher = Phase3UiDispatcher::new(&guard, &adapter);
        let window = dispatcher
            .create_window(WindowOptions::new("Notes", size).expect("options"))
            .expect("create window");
        let orphan = WidgetNode::new(WidgetId::new(3).expect("orphan"), WidgetKind::Text);

        let err = dispatcher
            .upsert_node(window, orphan)
            .expect_err("missing widget tree should fail");

        assert!(matches!(
            err,
            UiDispatchError::Adapter(UiAdapterError::MissingWidgetTree { .. })
        ));
    }

    #[test]
    fn computes_layout_for_stored_widget_tree() {
        let guard = UapiGuard::new(SessionPolicy::default());
        let adapter = DraftUiAdapter::default();
        let dispatcher = Phase3UiDispatcher::new(&guard, &adapter);
        let window = dispatcher
            .create_window(
                WindowOptions::new("Notes", WindowSize::new(300, 200).expect("window size"))
                    .expect("options"),
            )
            .expect("create window");
        let root = WidgetNode::new(WidgetId::new(1).expect("root"), WidgetKind::Stack);
        let row = WidgetNode::new(WidgetId::new(2).expect("row"), WidgetKind::Text)
            .with_parent(root.id)
            .with_style(WidgetStyle {
                width: Some(120.0),
                height: Some(32.0),
                ..WidgetStyle::default()
            })
            .expect("style");

        dispatcher.set_root(window, root).expect("root");
        dispatcher.upsert_node(window, row).expect("row");
        let layout = dispatcher
            .compute_layout(window, LayoutViewport::new(300.0, 200.0).expect("viewport"))
            .expect("layout");

        assert_eq!(
            layout.rect(WidgetId::new(1).expect("root")),
            Some(layer36_layout::ComputedRect {
                x: 0.0,
                y: 0.0,
                width: 300.0,
                height: 200.0,
            })
        );
        assert_eq!(
            layout.rect(WidgetId::new(2).expect("row")),
            Some(layer36_layout::ComputedRect {
                x: 0.0,
                y: 0.0,
                width: 120.0,
                height: 32.0,
            })
        );
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
