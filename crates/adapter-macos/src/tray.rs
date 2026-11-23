//! A tray icon on a Mac (IC-1003): an NSStatusItem in the menu bar with
//! the app's own picture (a template image when asked, so it matches the bar
//! in light and dark mode), a tooltip, text beside it, and either a menu
//! (built by menus.rs, so its items are the same as a menu bar's) or clicks
//! that go to the runtime as signals.

use std::cell::RefCell;
use std::collections::BTreeMap;

use krate_adapter_common::menus::{push_signal, MenuSignal, MenuTree, TrayIcon};
use objc2::rc::{autoreleasepool, Retained};
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{define_class, msg_send, sel, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSEventMask};
use objc2_foundation::{NSDefaultRunLoopMode, NSObject, NSObjectProtocol, NSSize, NSString};

/// How tall the icon stands in the menu bar, in points.
const ICON_POINTS: f64 = 18.0;

struct Item {
    status: Retained<AnyObject>,
}

thread_local! {
    static ITEMS: RefCell<BTreeMap<u64, Item>> = const { RefCell::new(BTreeMap::new()) };
    static NEXT: RefCell<u64> = const { RefCell::new(1) };
    static TARGET: RefCell<Option<Retained<KrateTrayTarget>>> = const { RefCell::new(None) };
}

define_class!(
    // SAFETY:
    // - NSObject has no subclassing requirements for a target-action receiver.
    // - AppKit sends a status item's action on the main thread.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    struct KrateTrayTarget;

    // SAFETY: NSObjectProtocol has no additional requirements.
    unsafe impl NSObjectProtocol for KrateTrayTarget {}

    impl KrateTrayTarget {
        // SAFETY: `-(void)action:(id)sender` from an NSStatusBarButton Krate
        // made, whose tag is the tray id.
        #[unsafe(method(krateTrayClicked:))]
        fn clicked(&self, sender: &AnyObject) {
            let tray: isize = unsafe { msg_send![sender, tag] };
            let Some(mtm) = MainThreadMarker::new() else {
                return;
            };
            let right = NSApplication::sharedApplication(mtm)
                .currentEvent()
                .is_some_and(|e| {
                    use objc2_app_kit::NSEventType;
                    matches!(e.r#type(), NSEventType::RightMouseUp | NSEventType::RightMouseDown)
                        || e.modifierFlags()
                            .contains(objc2_app_kit::NSEventModifierFlags::Control)
                });
            push_signal(MenuSignal::TrayClick {
                tray: tray as u64,
                right,
            });
        }
    }
);

fn target(mtm: MainThreadMarker) -> Retained<KrateTrayTarget> {
    TARGET.with(|t| {
        t.borrow_mut()
            .get_or_insert_with(|| {
                let this = KrateTrayTarget::alloc(mtm).set_ivars(());
                // SAFETY: NSObject's init on a fresh allocation.
                unsafe { msg_send![super(this), init] }
            })
            .clone()
    })
}

/// An NSImage of the app's pixels, `ICON_POINTS` tall.
fn image(icon: &TrayIcon) -> Option<Retained<AnyObject>> {
    // SAFETY: an NSBitmapImageRep Krate allocates and fills with exactly
    // width x height x 4 bytes (checked by TrayIcon::validate), wrapped in
    // an NSImage.
    unsafe {
        let rep_class = AnyClass::get(c"NSBitmapImageRep")?;
        let image_class = AnyClass::get(c"NSImage")?;
        let rep: objc2::rc::Allocated<AnyObject> = msg_send![rep_class, alloc];
        let null: *mut *mut u8 = std::ptr::null_mut();
        let space = NSString::from_str("NSDeviceRGBColorSpace");
        // NSBitmapFormatAlphaNonpremultiplied: the app's pixels are straight.
        let rep: Option<Retained<AnyObject>> = msg_send![rep,
            initWithBitmapDataPlanes: null,
            pixelsWide: icon.width as isize,
            pixelsHigh: icon.height as isize,
            bitsPerSample: 8isize,
            samplesPerPixel: 4isize,
            hasAlpha: true,
            isPlanar: false,
            colorSpaceName: &*space,
            bitmapFormat: 2usize,
            bytesPerRow: (icon.width * 4) as isize,
            bitsPerPixel: 32isize];
        let rep = rep?;
        let data: *mut u8 = msg_send![&*rep, bitmapData];
        if data.is_null() {
            return None;
        }
        std::ptr::copy_nonoverlapping(icon.rgba.as_ptr(), data, icon.rgba.len());
        let width = ICON_POINTS * icon.width as f64 / icon.height as f64;
        let made: objc2::rc::Allocated<AnyObject> = msg_send![image_class, alloc];
        let image: Retained<AnyObject> =
            msg_send![made, initWithSize: NSSize::new(width, ICON_POINTS)];
        let _: () = msg_send![&*image, addRepresentation: &*rep];
        let _: () = msg_send![&*image, setTemplate: icon.template];
        Some(image)
    }
}

fn button(item: &AnyObject) -> Option<Retained<AnyObject>> {
    // SAFETY: NSStatusItem's button.
    unsafe { msg_send![item, button] }
}

/// Show an icon; its id.
pub(crate) fn show(icon: &TrayIcon, tooltip: &str) -> Result<u64, String> {
    let mtm = MainThreadMarker::new().ok_or("a tray icon is made on the main thread")?;
    let app = NSApplication::sharedApplication(mtm);
    // An app with no window yet has no activation policy that shows
    // anything; as a menu bar app it needs no Dock icon either.
    if app.activationPolicy() == NSApplicationActivationPolicy::Prohibited {
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    }
    let picture = image(icon).ok_or("the icon could not be drawn")?;
    let id = NEXT.with(|n| {
        let id = *n.borrow();
        *n.borrow_mut() += 1;
        id
    });
    let target = target(mtm);
    // SAFETY: the system status bar and a variable-length item in it; the
    // button's image, tooltip, tag, target and action, on the main thread.
    unsafe {
        let bar_class = AnyClass::get(c"NSStatusBar").ok_or("this Mac has no status bar")?;
        let bar: Retained<AnyObject> = msg_send![bar_class, systemStatusBar];
        // NSVariableStatusItemLength.
        let status: Retained<AnyObject> = msg_send![&*bar, statusItemWithLength: -1.0f64];
        let b = button(&status).ok_or("the status item has no button")?;
        let _: () = msg_send![&*b, setImage: &*picture];
        // NSImageLeft: the title, when there is one, follows the icon.
        let _: () = msg_send![&*b, setImagePosition: 2isize];
        let _: () = msg_send![&*b, setToolTip: &*NSString::from_str(tooltip)];
        let _: () = msg_send![&*b, setTag: id as isize];
        let _: () = msg_send![&*b, setTarget: &*target];
        let _: () = msg_send![&*b, setAction: sel!(krateTrayClicked:)];
        // Left and right mouse up both reach the action.
        let mask = NSEventMask::LeftMouseUp | NSEventMask::RightMouseUp;
        let _: isize = msg_send![&*b, sendActionOn: mask];
        ITEMS.with(|items| items.borrow_mut().insert(id, Item { status }));
    }
    Ok(id)
}

fn with_item<T>(tray: u64, f: impl FnOnce(&Item) -> T) -> Option<T> {
    ITEMS.with(|items| items.borrow().get(&tray).map(f))
}

pub(crate) fn set_icon(tray: u64, icon: &TrayIcon) -> Result<(), String> {
    let picture = image(icon).ok_or("the icon could not be drawn")?;
    with_item(tray, |item| {
        if let Some(b) = button(&item.status) {
            // SAFETY: an NSStatusBarButton's image, main thread.
            unsafe {
                let _: () = msg_send![&*b, setImage: &*picture];
            }
        }
    })
    .ok_or_else(|| "no such tray icon".to_string())
}

pub(crate) fn set_tooltip(tray: u64, tooltip: &str) -> Result<(), String> {
    with_item(tray, |item| {
        if let Some(b) = button(&item.status) {
            // SAFETY: an NSStatusBarButton's tooltip, main thread.
            unsafe {
                let _: () = msg_send![&*b, setToolTip: &*NSString::from_str(tooltip)];
            }
        }
    })
    .ok_or_else(|| "no such tray icon".to_string())
}

pub(crate) fn set_title(tray: u64, title: &str) -> Result<(), String> {
    with_item(tray, |item| {
        if let Some(b) = button(&item.status) {
            // SAFETY: an NSStatusBarButton's title, main thread.
            unsafe {
                let _: () = msg_send![&*b, setTitle: &*NSString::from_str(title)];
            }
        }
    })
    .ok_or_else(|| "no such tray icon".to_string())
}

pub(crate) fn set_menu(tray: u64, tree: Option<&MenuTree>) -> Result<(), String> {
    let menu = match tree {
        Some(tree) => {
            Some(crate::menus::tray_menu(tray, tree).ok_or("menus are built on the main thread")?)
        }
        None => {
            crate::menus::forget_tray(tray);
            None
        }
    };
    with_item(tray, |item| {
        // SAFETY: NSStatusItem's menu: with one, any click opens it.
        unsafe {
            let _: () = msg_send![&*item.status, setMenu: menu.as_deref()];
        }
    })
    .ok_or_else(|| "no such tray icon".to_string())
}

pub(crate) fn remove(tray: u64) {
    let Some(item) = ITEMS.with(|items| items.borrow_mut().remove(&tray)) else {
        return;
    };
    crate::menus::forget_tray(tray);
    // SAFETY: the system status bar takes its item back, main thread.
    unsafe {
        if let Some(bar_class) = AnyClass::get(c"NSStatusBar") {
            let bar: Retained<AnyObject> = msg_send![bar_class, systemStatusBar];
            let _: () = msg_send![&*bar, removeStatusItem: &*item.status];
        }
    }
}

/// Wait inside AppKit's own run loop until an event arrives or `max`
/// passes, leaving the event queued for the pump. A blind sleep there kept
/// the run loop stopped: a web view's process could not answer until the
/// next pump, so a key typed into a wrapped web app showed a quarter of a
/// second late (IC-1004), and every click waited out the rest of the nap.
pub(crate) fn park(max: std::time::Duration) -> bool {
    let Some(mtm) = MainThreadMarker::new() else {
        return false;
    };
    let app = NSApplication::sharedApplication(mtm);
    autoreleasepool(|_| {
        let until = objc2_foundation::NSDate::dateWithTimeIntervalSinceNow(max.as_secs_f64());
        // SAFETY: main thread; the event is peeked, not taken, so the pump
        // still sees it.
        let _ = unsafe {
            app.nextEventMatchingMask_untilDate_inMode_dequeue(
                NSEventMask::Any,
                Some(&until),
                NSDefaultRunLoopMode,
                false,
            )
        };
    });
    true
}

/// Dispatch the Mac's events with no window open, so a tray icon's clicks
/// and menu reach the app.
pub(crate) fn pump() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    autoreleasepool(|_| loop {
        // SAFETY: main thread; a nil date never waits.
        let event = unsafe {
            app.nextEventMatchingMask_untilDate_inMode_dequeue(
                NSEventMask::Any,
                None,
                NSDefaultRunLoopMode,
                true,
            )
        };
        let Some(event) = event else {
            break;
        };
        app.sendEvent(&event);
    });
}
