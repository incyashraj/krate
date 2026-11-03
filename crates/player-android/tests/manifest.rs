//! The player activity's manifest keeps the running app alive through what
//! a phone does in a person's hands.
//!
//! Android restarts an activity for every configuration change its manifest
//! does not name, and restarting Krate's activity ends the app inside it,
//! with whatever the person had not saved. A keyboard plugged into a tablet,
//! a foldable opened, a split screen resized, the language changed: each one
//! not listed here is an app lost.

fn activity_attribute(name: &str) -> String {
    let manifest = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/android/AndroidManifest.xml"
    ))
    .expect("the player's manifest");
    let activity = manifest
        .split("<activity")
        .find(|block| block.contains("tech.krate.player.KrateActivity"))
        .expect("the player activity");
    let activity = &activity[..activity.find('>').expect("the activity tag closes")];
    let key = format!("android:{name}=\"");
    let start = activity.find(&key).unwrap_or_else(|| panic!("no {name}")) + key.len();
    let end = start + activity[start..].find('"').expect("the value closes");
    activity[start..end].to_string()
}

#[test]
fn the_running_app_survives_every_change_a_phone_makes() {
    let handled = activity_attribute("configChanges");
    let handled: Vec<&str> = handled.split('|').collect();
    for change in [
        // Turning the phone.
        "orientation",
        "screenSize",
        // Split screen, a foldable, a tablet's free windows.
        "smallestScreenSize",
        "screenLayout",
        // A keyboard plugged in or slid out, and the on-screen one.
        "keyboard",
        "keyboardHidden",
        "navigation",
        "touchscreen",
        // A display moved to, dark mode, text size, the language.
        "density",
        "uiMode",
        "fontScale",
        "locale",
        "layoutDirection",
    ] {
        assert!(
            handled.contains(&change),
            "a {change} change would restart the activity and end the app"
        );
    }
}

#[test]
fn the_keyboard_is_hidden_until_asked_and_reported_as_a_covered_edge() {
    let mode = activity_attribute("windowSoftInputMode");
    let parts: Vec<&str> = mode.split('|').collect();
    assert!(parts.contains(&"adjustResize"), "{mode}");
    assert!(parts.contains(&"stateHidden"), "{mode}");
}
