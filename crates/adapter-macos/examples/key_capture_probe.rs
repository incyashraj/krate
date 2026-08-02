//! Prove a keypress travels the production path from NSApp queue to runtime.
//!
//! Posts a synthetic Space key-down into this process's own AppKit event
//! queue, pumps once through the exact code `krate run` uses, and asserts the
//! sample comes out the runtime side with the portable name apps hard-code.
//!
//! An example rather than a test because AppKit only works on the process
//! main thread, and cargo's test harness runs tests on worker threads; an
//! example binary's `main` is the real main thread. No accessibility
//! permission is needed: posting into your own queue is not synthesizing
//! global input.
//!
//!     cargo run -p krate-adapter-macos --example key_capture_probe

use krate_adapter_common::ui::{UiAdapter, WindowAdapter, WindowOptions, WindowSize};

fn main() {
    let adapter = krate_adapter_macos::discover_appkit_prototype_ui_adapter()
        .expect("AppKit prototype adapter");

    let options = WindowOptions::new(
        "Key capture probe",
        WindowSize::new(320, 200).expect("size"),
    )
    .expect("options");
    let id = WindowAdapter::create_window(&adapter, options).expect("create window");
    WindowAdapter::show_window(&adapter, id).expect("show window");

    post_key(49, " ", true);
    post_key(49, " ", false);

    // A posted event can take a runloop turn to surface, so pump a few times
    // and collect, the way the real frame loop pumps every 16 milliseconds.
    let mut samples = Vec::new();
    for _ in 0..10 {
        let _ = UiAdapter::pump_event_loop_once(&adapter, id);
        samples.extend(adapter.drain_raw_key_input());
        let down_seen = samples.iter().any(|s| s.key == "Space" && s.pressed);
        let up_seen = samples.iter().any(|s| s.key == "Space" && !s.pressed);
        if down_seen && up_seen {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(16));
    }

    let _ = WindowAdapter::close_window(&adapter, id);

    let down_ok = samples.iter().any(|s| s.key == "Space" && s.pressed);
    let up_ok = samples.iter().any(|s| s.key == "Space" && !s.pressed);
    if down_ok && up_ok {
        println!("KEY CAPTURE OK: Space down and up both reached the runtime path");
    } else {
        println!("KEY CAPTURE BROKEN: samples={samples:?}");
        std::process::exit(1);
    }
}

/// Post one synthetic key event into this process's AppKit queue.
fn post_key(key_code: u16, characters: &str, down: bool) {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSEvent, NSEventModifierFlags, NSEventType};
    use objc2_foundation::{NSPoint, NSString};

    let mtm = MainThreadMarker::new().expect("main thread");
    let app = NSApplication::sharedApplication(mtm);
    let window_number = app
        .keyWindow()
        .map(|window| window.windowNumber())
        .unwrap_or(0);
    let event_type = if down {
        NSEventType::KeyDown
    } else {
        NSEventType::KeyUp
    };
    let characters = NSString::from_str(characters);
    let event = {
        NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
            event_type,
            NSPoint::new(0.0, 0.0),
            NSEventModifierFlags::empty(),
            0.0,
            window_number,
            None,
            &characters,
            &characters,
            false,
            key_code,
        )
    }
    .expect("synthetic key event");
    app.postEvent_atStart(&event, false);
}
