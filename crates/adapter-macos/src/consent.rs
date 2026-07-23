//! The macOS native consent window (P3-OPEN-01).
//!
//! When a `.krate` is opened without pre-supplied grants — the double-click
//! path, where there is no terminal to answer — this presents a modal window
//! naming the app and listing each requested capability with the author's
//! rationale and a per-capability checkbox. The person allows a subset and
//! presses Open, or presses Cancel and nothing runs.
//!
//! This window is macOS-only for now (founder decision, 2026-07-23). Linux and
//! Windows keep the terminal consent prompt; the CLI selects between them and
//! falls back to the terminal when [`present_consent_window`] reports the
//! platform is unsupported. The non-macOS build of this module is a stub that
//! compiles and returns that fallback signal, so the off-macOS path is exercised
//! on those CI lanes rather than left to break a later build.

use krate_adapter_common::ui::UiAdapterError;

/// One capability shown as a row in the consent window.
#[derive(Debug, Clone)]
pub struct ConsentItem {
    /// The capability string as the person should read it (e.g. the grant).
    pub display: String,
    /// The author's own reason for requesting it, from the manifest.
    pub rationale: String,
    /// Whether the app cannot run without it. Required rows start checked.
    pub required: bool,
}

/// What the person decided in the consent window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsentChoice {
    /// They pressed Open; the indices are the rows they left checked, into the
    /// `items` slice passed to [`present_consent_window`].
    Open(Vec<usize>),
    /// They pressed Cancel; nothing is granted and the run is refused.
    Cancel,
}

/// Present the consent window and block until the person decides.
///
/// On macOS this runs a modal AppKit window on the main thread. On every other
/// platform it returns `Unsupported` via an error the CLI treats as "fall back
/// to the terminal prompt", so `--consent` still works there.
#[cfg(target_os = "macos")]
pub fn present_consent_window(
    app_name: &str,
    app_id: &str,
    items: &[ConsentItem],
) -> Result<ConsentChoice, UiAdapterError> {
    platform::present(app_name, app_id, items)
}

#[cfg(not(target_os = "macos"))]
pub fn present_consent_window(
    _app_name: &str,
    _app_id: &str,
    _items: &[ConsentItem],
) -> Result<ConsentChoice, UiAdapterError> {
    Err(UiAdapterError::Unsupported(
        "the native consent window is only available on macOS".to_string(),
    ))
}

#[cfg(target_os = "macos")]
mod platform {
    use super::{ConsentChoice, ConsentItem};
    use krate_adapter_common::ui::UiAdapterError;
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{
        NSApplication, NSBackingStoreType, NSButton, NSColor, NSControlStateValueOff,
        NSControlStateValueOn, NSFont, NSTextField, NSView, NSWindow, NSWindowStyleMask,
    };
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
    use std::cell::Cell;
    use std::rc::Rc;

    /// Modal return codes. Distinct so the caller can tell Open from Cancel;
    /// AppKit reserves nothing in this range for its own responses here.
    const RESPONSE_OPEN: isize = 1000;
    const RESPONSE_CANCEL: isize = 1001;

    // Window geometry. Fixed width, height grows with the row count up to a cap.
    const WINDOW_WIDTH: f64 = 460.0;
    const ROW_HEIGHT: f64 = 52.0;
    const HEADER_HEIGHT: f64 = 72.0;
    const FOOTER_HEIGHT: f64 = 64.0;
    const MAX_LIST_HEIGHT: f64 = 360.0;

    struct ButtonTargetIvars {
        // Which button was pressed, written by the action and read after the
        // modal loop ends.
        response: Rc<Cell<isize>>,
    }

    define_class!(
        // A tiny target object for the Open and Cancel buttons: it records the
        // response code and stops the modal loop so control returns to `present`.
        #[unsafe(super(objc2::runtime::NSObject))]
        #[thread_kind = MainThreadOnly]
        #[ivars = ButtonTargetIvars]
        struct ConsentButtonTarget;

        impl ConsentButtonTarget {
            #[unsafe(method(open:))]
            fn open(&self, _sender: Option<&AnyObject>) {
                self.ivars().response.set(RESPONSE_OPEN);
                stop_modal(self.mtm(), RESPONSE_OPEN);
            }

            #[unsafe(method(cancel:))]
            fn cancel(&self, _sender: Option<&AnyObject>) {
                self.ivars().response.set(RESPONSE_CANCEL);
                stop_modal(self.mtm(), RESPONSE_CANCEL);
            }
        }
    );

    impl ConsentButtonTarget {
        fn new(mtm: MainThreadMarker, response: Rc<Cell<isize>>) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(ButtonTargetIvars { response });
            unsafe { msg_send![super(this), init] }
        }
    }

    fn stop_modal(mtm: MainThreadMarker, code: isize) {
        let app = NSApplication::sharedApplication(mtm);
        app.stopModalWithCode(code);
    }

    pub(super) fn present(
        app_name: &str,
        app_id: &str,
        items: &[ConsentItem],
    ) -> Result<ConsentChoice, UiAdapterError> {
        let mtm = MainThreadMarker::new().ok_or_else(|| {
            UiAdapterError::Unsupported(
                "the consent window must be shown on the macOS main thread".to_string(),
            )
        })?;

        let list_height = (items.len() as f64 * ROW_HEIGHT).min(MAX_LIST_HEIGHT);
        let window_height = HEADER_HEIGHT + list_height + FOOTER_HEIGHT;

        let style = NSWindowStyleMask::Titled | NSWindowStyleMask::Closable;
        let rect = NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(WINDOW_WIDTH, window_height),
        );
        // SAFETY: NSWindow allocation and init must happen on the main thread;
        // the MainThreadMarker above proves we are on it. The window is retained
        // for the duration of the modal loop below.
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                rect,
                style,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        window.setTitle(&NSString::from_str("Open with Krate"));
        window.center();

        let content = NSView::initWithFrame(NSView::alloc(mtm), rect);

        // Header: app identity and a one-line explanation of the wall.
        let header_y = window_height - HEADER_HEIGHT;
        let title = bold_label(
            mtm,
            &format!("{app_name} wants to open"),
            NSRect::new(
                NSPoint::new(20.0, header_y + 38.0),
                NSSize::new(WINDOW_WIDTH - 40.0, 22.0),
            ),
        );
        content.addSubview(&title);
        let subtitle = muted_label(
            mtm,
            &format!("{app_id} · it can only touch what you allow below"),
            NSRect::new(
                NSPoint::new(20.0, header_y + 14.0),
                NSSize::new(WINDOW_WIDTH - 40.0, 18.0),
            ),
        );
        content.addSubview(&subtitle);

        // One checkbox per capability, with its rationale beneath.
        let mut checkboxes: Vec<Retained<NSButton>> = Vec::with_capacity(items.len());
        let list_top = HEADER_HEIGHT + list_height;
        for (index, item) in items.iter().enumerate() {
            let row_top = list_top - (index as f64 * ROW_HEIGHT);
            let checkbox_rect = NSRect::new(
                NSPoint::new(20.0, row_top - 22.0),
                NSSize::new(WINDOW_WIDTH - 40.0, 20.0),
            );
            let label = if item.required {
                format!("{}  (required)", item.display)
            } else {
                item.display.clone()
            };
            // SAFETY: checkbox construction needs the main thread; held above.
            let checkbox = unsafe {
                NSButton::checkboxWithTitle_target_action(
                    &NSString::from_str(&label),
                    None,
                    None,
                    mtm,
                )
            };
            checkbox.setFrame(checkbox_rect);
            // Required capabilities start checked; optional ones start off so
            // the person opts in rather than out.
            checkbox.setState(if item.required {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
            content.addSubview(&checkbox);
            checkboxes.push(checkbox);

            if !item.rationale.is_empty() {
                let rationale = muted_label(
                    mtm,
                    &item.rationale,
                    NSRect::new(
                        NSPoint::new(38.0, row_top - 42.0),
                        NSSize::new(WINDOW_WIDTH - 58.0, 16.0),
                    ),
                );
                content.addSubview(&rationale);
            }
        }

        // Footer: Cancel on the left, Open on the right.
        let response = Rc::new(Cell::new(RESPONSE_CANCEL));
        let target = ConsentButtonTarget::new(mtm, Rc::clone(&response));

        // SAFETY: button construction on the main thread; target outlives the
        // modal loop because it is held on the stack until `present` returns.
        let open_button = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str("Open"),
                Some(&target),
                Some(sel!(open:)),
                mtm,
            )
        };
        open_button.setFrame(NSRect::new(
            NSPoint::new(WINDOW_WIDTH - 120.0, 16.0),
            NSSize::new(100.0, 32.0),
        ));
        content.addSubview(&open_button);

        let cancel_button = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str("Cancel"),
                Some(&target),
                Some(sel!(cancel:)),
                mtm,
            )
        };
        cancel_button.setFrame(NSRect::new(
            NSPoint::new(20.0, 16.0),
            NSSize::new(100.0, 32.0),
        ));
        content.addSubview(&cancel_button);

        window.setContentView(Some(&content));

        let app = NSApplication::sharedApplication(mtm);
        // Matches the crate's existing window-activation path; the replacement
        // NSApp.activate is not yet in the pinned objc2-app-kit.
        #[allow(deprecated)]
        app.activateIgnoringOtherApps(true);
        window.makeKeyAndOrderFront(None);

        // Block here until a button stops the modal loop. NSModalResponse is a
        // plain NSInteger, so this is the raw code the button set.
        let code = app.runModalForWindow(&window);
        window.close();

        if code != RESPONSE_OPEN {
            return Ok(ConsentChoice::Cancel);
        }

        let allowed = checkboxes
            .iter()
            .enumerate()
            .filter(|(_, checkbox)| checkbox.state() == NSControlStateValueOn)
            .map(|(index, _)| index)
            .collect();
        Ok(ConsentChoice::Open(allowed))
    }

    fn bold_label(mtm: MainThreadMarker, text: &str, frame: NSRect) -> Retained<NSTextField> {
        let label = NSTextField::labelWithString(&NSString::from_str(text), mtm);
        label.setFrame(frame);
        let size = NSFont::systemFontSize();
        label.setFont(Some(&NSFont::boldSystemFontOfSize(size)));
        label
    }

    fn muted_label(mtm: MainThreadMarker, text: &str, frame: NSRect) -> Retained<NSTextField> {
        let label = NSTextField::labelWithString(&NSString::from_str(text), mtm);
        label.setFrame(frame);
        label.setTextColor(Some(&NSColor::secondaryLabelColor()));
        let small = NSFont::smallSystemFontSize();
        label.setFont(Some(&NSFont::systemFontOfSize(small)));
        label
    }
}
