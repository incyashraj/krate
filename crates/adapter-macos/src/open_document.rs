//! Receive the macOS open-document event (P3-OPEN-03).
//!
//! When Finder opens a `.krate` (double-click, or `open file.krate`), Launch
//! Services does not pass the path in argv — it delivers an Apple event to the
//! launched app. This module runs a minimal `NSApplication` launch sequence
//! with a delegate that captures `application:openFiles:`, stops the loop as
//! soon as launching finishes, and hands the captured paths back to the CLI,
//! which then runs the ordinary consent + run flow on them.
//!
//! macOS-only by nature; the non-macOS build is a stub that reports
//! unsupported, so the crate keeps compiling everywhere it is referenced.

use krate_adapter_common::ui::UiAdapterError;
use std::path::PathBuf;

/// Block until macOS finishes launching us and delivers any opened documents.
///
/// Returns the documents Launch Services asked us to open. An empty vec means
/// the app was launched directly (double-clicked Krate.app itself, no file).
///
/// `late_open` stays installed for the life of the process: if Finder sends
/// another document to this already-running instance later (double-click while
/// an app is open), that handler receives it. Without this, macOS shows the
/// "Krate cannot open files in this format" alert, because the running
/// instance would otherwise refuse the event.
#[cfg(target_os = "macos")]
pub fn wait_for_opened_documents(
    late_open: Box<dyn Fn(PathBuf)>,
) -> Result<Vec<PathBuf>, UiAdapterError> {
    platform::wait(late_open)
}

#[cfg(not(target_os = "macos"))]
pub fn wait_for_opened_documents(
    _late_open: Box<dyn Fn(PathBuf)>,
) -> Result<Vec<PathBuf>, UiAdapterError> {
    Err(UiAdapterError::Unsupported(
        "open-document events are only delivered on macOS".to_string(),
    ))
}

#[cfg(target_os = "macos")]
mod platform {
    use krate_adapter_common::ui::UiAdapterError;
    use objc2::rc::Retained;
    use objc2::runtime::ProtocolObject;
    use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{NSApplication, NSApplicationDelegate};
    use objc2_foundation::{NSArray, NSNotification, NSObject, NSObjectProtocol, NSString};
    use std::cell::RefCell;
    use std::path::PathBuf;
    use std::rc::Rc;

    type SharedPaths = Rc<RefCell<Vec<PathBuf>>>;
    type LateHandler = Rc<RefCell<Option<Box<dyn Fn(PathBuf)>>>>;

    struct OpenDelegateIvars {
        opened: SharedPaths,
        /// Set once launching finishes; later documents go to `late` instead.
        launched: Rc<std::cell::Cell<bool>>,
        late: LateHandler,
    }

    define_class!(
        // SAFETY:
        // - NSObject has no extra subclassing requirements for a passive delegate.
        // - Main-thread-only because AppKit delivers these callbacks there.
        #[unsafe(super = NSObject)]
        #[thread_kind = MainThreadOnly]
        #[ivars = OpenDelegateIvars]
        struct KrateOpenDelegate;

        // SAFETY: NSObjectProtocol has no additional safety requirements.
        unsafe impl NSObjectProtocol for KrateOpenDelegate {}

        // SAFETY: Method signatures match the generated NSApplicationDelegate
        // protocol.
        unsafe impl NSApplicationDelegate for KrateOpenDelegate {
            #[unsafe(method(application:openFiles:))]
            fn application_open_files(&self, _sender: &NSApplication, files: &NSArray<NSString>) {
                let ivars = self.ivars();
                if ivars.launched.get() {
                    // This instance is already running an app; a new document
                    // gets its own process via the installed handler.
                    if let Some(handler) = ivars.late.borrow().as_ref() {
                        for file in files.to_vec() {
                            handler(PathBuf::from(file.to_string()));
                        }
                    }
                    return;
                }
                let mut opened = ivars.opened.borrow_mut();
                for file in files.to_vec() {
                    opened.push(PathBuf::from(file.to_string()));
                }
            }

            // Launch Services delivers the open event during launch, so by the
            // time launching has finished we either have the paths or there
            // were none. Stop the loop and give control back to the CLI.
            #[unsafe(method(applicationDidFinishLaunching:))]
            fn application_did_finish_launching(&self, _notification: &NSNotification) {
                self.ivars().launched.set(true);
                let app = NSApplication::sharedApplication(self.mtm());
                app.stop(None);
            }
        }
    );

    impl KrateOpenDelegate {
        fn new(
            mtm: MainThreadMarker,
            opened: SharedPaths,
            launched: Rc<std::cell::Cell<bool>>,
            late: LateHandler,
        ) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(OpenDelegateIvars {
                opened,
                launched,
                late,
            });
            // SAFETY: NSObject's `init` signature is correct for a fresh allocation.
            unsafe { msg_send![super(this), init] }
        }
    }

    pub(super) fn wait(late_open: Box<dyn Fn(PathBuf)>) -> Result<Vec<PathBuf>, UiAdapterError> {
        let mtm = MainThreadMarker::new().ok_or_else(|| {
            UiAdapterError::Unsupported(
                "open-document events must be received on the macOS main thread".to_string(),
            )
        })?;

        let opened: SharedPaths = Rc::new(RefCell::new(Vec::new()));
        let launched = Rc::new(std::cell::Cell::new(false));
        let late: LateHandler = Rc::new(RefCell::new(Some(late_open)));
        let delegate =
            KrateOpenDelegate::new(mtm, Rc::clone(&opened), Rc::clone(&launched), late);
        let app = NSApplication::sharedApplication(mtm);
        app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));

        // Runs the launch sequence: the odoc Apple event (if any) is delivered
        // to the delegate, then applicationDidFinishLaunching stops the loop.
        app.run();

        // The delegate stays installed for the life of the process so a later
        // document sent to this running instance reaches the late handler
        // instead of being refused with an OS alert. Keeping it alive forever
        // is intentional: one delegate per process, freed at exit.
        std::mem::forget(delegate);

        let paths = opened.borrow().clone();
        Ok(paths)
    }
}
