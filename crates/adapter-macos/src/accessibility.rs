//! What VoiceOver reads on a Mac for an app that draws itself (IC-926):
//! one NSAccessibilityElement per element the app described, children of
//! the window's content view, framed in that view's own space so they move
//! with the window. VoiceOver's press on one reaches the app as a click at
//! its centre, as a double-tap does on an iPhone.

use std::cell::RefCell;
use std::collections::BTreeMap;

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool};
use objc2::{define_class, msg_send, AllocAnyThread, DefinedClass};
use objc2_app_kit::NSAccessibilityElement;
use objc2_foundation::{NSArray, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString};

use krate_adapter_common::ui::{AccessElement, AccessRole, RawPointerSample, WindowId};

/// Where a press lands: the app's window and the element's centre.
struct Press {
    window: WindowId,
    x: f32,
    y: f32,
}

define_class!(
    // SAFETY:
    // - NSAccessibilityElement may be subclassed to answer actions.
    // - AppKit asks it on the main thread.
    #[unsafe(super = NSAccessibilityElement)]
    #[ivars = Press]
    struct KrateMacAccessElement;

    // SAFETY: NSObjectProtocol has no additional requirements.
    unsafe impl NSObjectProtocol for KrateMacAccessElement {}

    impl KrateMacAccessElement {
        /// VoiceOver pressed it: a click at its centre, down then up.
        #[unsafe(method(accessibilityPerformPress))]
        fn press(&self) -> Bool {
            let at = self.ivars();
            for pressed in [true, false] {
                crate::appkit::push_raw_pointer_sample(RawPointerSample {
                    window: at.window,
                    x: at.x,
                    y: at.y,
                    pressed,
                });
            }
            Bool::YES
        }
    }
);

thread_local! {
    /// Each window's elements, kept alive while VoiceOver may ask for them.
    static HELD: RefCell<BTreeMap<u64, Vec<Retained<KrateMacAccessElement>>>> =
        const { RefCell::new(BTreeMap::new()) };
}

fn role_name(role: AccessRole) -> &'static str {
    match role {
        AccessRole::Button => "AXButton",
        AccessRole::Link => "AXLink",
        AccessRole::Heading => "AXHeading",
        AccessRole::Text => "AXStaticText",
        AccessRole::Image => "AXImage",
        AccessRole::Toggle => "AXCheckBox",
        AccessRole::Adjustable => "AXSlider",
        AccessRole::Field => "AXTextField",
    }
}

/// Tell VoiceOver what the window whose NSWindow is `ns_window` shows.
/// Main thread (a Mac app runs on it).
pub(crate) fn describe(ns_window: u64, window: WindowId, elements: &[AccessElement]) {
    if objc2::MainThreadMarker::new().is_none() {
        return;
    }
    // Drained here: the runtime pumps its own event loop, so whatever the
    // calls below autorelease would otherwise wait for a pool that never
    // drains, one description after another (K-1374).
    objc2::rc::autoreleasepool(|_| describe_pooled(ns_window, window, elements));
}

fn describe_pooled(ns_window: u64, window: WindowId, elements: &[AccessElement]) {
    // SAFETY: the NSWindow the adapter made for this app (its pointer is the
    // native handle); its content view's bounds; accessibility elements made
    // and set as that view's children, on the main thread.
    unsafe {
        let Some(win) = Retained::retain(ns_window as usize as *mut AnyObject) else {
            return;
        };
        let content: Option<Retained<AnyObject>> = msg_send![&*win, contentView];
        let Some(content) = content else {
            return;
        };
        let bounds: NSRect = msg_send![&*content, bounds];
        let flipped: bool = msg_send![&*content, isFlipped];
        let mut made = Vec::with_capacity(elements.len());
        for e in elements {
            let element = KrateMacAccessElement::alloc().set_ivars(Press {
                window,
                x: e.x + e.width / 2.0,
                y: e.y + e.height / 2.0,
            });
            let element: Retained<KrateMacAccessElement> = msg_send![super(element), init];
            let y = if flipped {
                e.y as f64
            } else {
                bounds.size.height - e.y as f64 - e.height as f64
            };
            let frame = NSRect::new(
                NSPoint::new(e.x as f64, y),
                NSSize::new(e.width as f64, e.height as f64),
            );
            let _: () = msg_send![&*element, setAccessibilityFrameInParentSpace: frame];
            let _: () = msg_send![&*element, setAccessibilityParent: &*content];
            let _: () =
                msg_send![&*element, setAccessibilityRole: &*NSString::from_str(role_name(e.role))];
            let _: () = msg_send![&*element, setAccessibilityLabel: &*NSString::from_str(&e.label)];
            let value = match e.checked {
                Some(true) => "On".to_string(),
                Some(false) => "Off".to_string(),
                None => e.value.clone(),
            };
            if !value.is_empty() {
                let _: () =
                    msg_send![&*element, setAccessibilityValue: &*NSString::from_str(&value)];
            }
            let _: () = msg_send![&*element, setAccessibilityEnabled: e.enabled];
            made.push(element);
        }
        let children: Vec<&AnyObject> = made.iter().map(|e| &***e as &AnyObject).collect();
        let array = NSArray::from_slice(&children);
        let _: () = msg_send![&*content, setAccessibilityChildren: &*array];
        HELD.with(|held| held.borrow_mut().insert(window.get(), made));
    }
}

/// Whether VoiceOver is on.
pub(crate) fn voice_over_on() -> bool {
    // SAFETY: NSWorkspace's shared instance and its VoiceOver flag.
    unsafe {
        AnyClass::get(c"NSWorkspace").is_some_and(|class| {
            let workspace: Retained<AnyObject> = msg_send![class, sharedWorkspace];
            let on: Bool = msg_send![&*workspace, isVoiceOverEnabled];
            on.as_bool()
        })
    }
}
