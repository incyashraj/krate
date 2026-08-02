//! Gamepad state behind the `ui.events` gamepad calls.
//!
//! The interface is real and the backend is not yet wired, which is a
//! deliberate order rather than an oversight.
//!
//! Reading a gamepad on Linux means `libudev`, a system library. Adding one is
//! the sequence that broke two releases this week — the file picker needed
//! Wayland headers in fifteen CI jobs, speech-to-text needed a newer libclang
//! in a container nobody could see, and the arm64 Linux build is still missing
//! from rc5 because of it. So the shape lands first, on its own, and the
//! dependency lands as its own change with its own container work and its own
//! release to verify.
//!
//! Until then every query answers as though no controller is plugged in, which
//! is exactly what an app must already handle: a person without a gamepad is
//! the common case, not an error. An app written against this today keeps
//! working unchanged when the backend arrives.

use std::collections::BTreeMap;

/// Buttons every controller has, whatever is printed on them.
///
/// Positional names because the same physical button is A on an Xbox pad and B
/// on a Nintendo one; an app asking for `south` gets the button under the
/// player's thumb on both.
const BUTTONS: &[&str] = &[
    "south",
    "east",
    "west",
    "north",
    "l1",
    "r1",
    "l2",
    "r2",
    "start",
    "select",
    "dpad-up",
    "dpad-down",
    "dpad-left",
    "dpad-right",
];

/// Sticks and triggers. Up and right are positive.
const AXES: &[&str] = &["left-x", "left-y", "right-x", "right-y", "l2", "r2"];

/// Gamepad state for one app session.
#[derive(Debug, Default)]
pub struct Gamepads {
    /// True once a backend reports a pad. Always false today.
    connected: bool,
    /// Buttons currently down, by the portable name.
    held: BTreeMap<String, bool>,
    /// Axis positions, by the portable name.
    axes: BTreeMap<String, f32>,
}

impl Gamepads {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn connected(&self) -> bool {
        self.connected
    }

    /// Whether a button is held. An unknown name is not held, rather than an
    /// error: an app checking `triangle` on a pad that calls it `north` should
    /// see nothing happen, not fall over.
    ///
    /// The name is checked against the documented list first, so a backend that
    /// starts reporting some pad-specific button cannot make it visible to apps
    /// without that name being added to the contract as well.
    pub fn held(&self, button: &str) -> bool {
        Self::is_known_button(button) && self.held.get(button).copied().unwrap_or(false)
    }

    /// A stick or trigger position. Zero with no gamepad, so an app that never
    /// checks `gamepad-connected` reads centred sticks rather than drift.
    pub fn axis(&self, axis: &str) -> f32 {
        if !Self::is_known_axis(axis) {
            return 0.0;
        }
        self.axes.get(axis).copied().unwrap_or(0.0)
    }

    /// Whether a name is one this interface defines.
    ///
    /// Used by the tests, and by any future backend to reject a name it should
    /// never produce rather than inventing a button an app cannot document.
    pub fn is_known_button(name: &str) -> bool {
        BUTTONS.contains(&name)
    }

    pub fn is_known_axis(name: &str) -> bool {
        AXES.contains(&name)
    }

    /// Set state directly, as a backend will once one exists.
    ///
    /// Test-only, because until then nothing else has any business writing
    /// here -- but the filtering in `held` and `axis` is unreachable without
    /// it, and unreachable code is untested code.
    #[cfg(test)]
    fn set(&mut self, button: &str, down: bool, axis: &str, position: f32) {
        self.connected = true;
        self.held.insert(button.to_string(), down);
        self.axes.insert(axis.to_string(), position);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_gamepad_reads_as_centred_and_unpressed() {
        // The state an app sees today, and the state it sees whenever somebody
        // has no controller plugged in -- which is the common case, not an
        // error case. Both must be quiet rather than surprising.
        let pads = Gamepads::new();
        assert!(!pads.connected());
        for button in BUTTONS {
            assert!(!pads.held(button), "{button} must not read as held");
        }
        for axis in AXES {
            assert_eq!(pads.axis(axis), 0.0, "{axis} must read centred");
        }
    }

    #[test]
    fn an_unknown_name_is_quiet_rather_than_an_error() {
        // An app asking for `triangle` on a pad whose north button is called
        // `north` should see nothing happen, not fall over.
        let pads = Gamepads::new();
        assert!(!pads.held("triangle"));
        assert_eq!(pads.axis("throttle"), 0.0);
    }

    #[test]
    fn an_off_contract_name_stays_invisible_even_when_a_backend_reports_it() {
        // The case the stub cannot reach on its own: some future backend
        // reports a pad-specific button, and an app starts depending on a name
        // that was never documented and will not exist on the next controller.
        // The lookup filters by the documented list so that cannot happen
        // quietly -- adding a button means adding it to the contract.
        let mut pads = Gamepads::new();
        pads.set("triangle", true, "throttle", 0.9);
        assert!(pads.connected(), "the backend did report a pad");
        assert!(
            !pads.held("triangle"),
            "an undocumented button stays hidden"
        );
        assert_eq!(
            pads.axis("throttle"),
            0.0,
            "an undocumented axis reads centred"
        );

        // And a documented name set the same way does come through, so the
        // test above is filtering rather than simply broken.
        let mut pads = Gamepads::new();
        pads.set("south", true, "left-x", 0.9);
        assert!(pads.held("south"));
        assert_eq!(pads.axis("left-x"), 0.9);
    }

    #[test]
    fn the_button_and_axis_names_are_the_ones_the_contract_documents() {
        // The WIT comment lists these names, and an app author will type them
        // exactly. A rename here without a rename there is a silent breakage.
        for name in [
            "south",
            "east",
            "west",
            "north",
            "l1",
            "r1",
            "l2",
            "r2",
            "start",
            "select",
            "dpad-up",
            "dpad-down",
            "dpad-left",
            "dpad-right",
        ] {
            assert!(Gamepads::is_known_button(name), "{name} is documented");
        }
        for name in ["left-x", "left-y", "right-x", "right-y", "l2", "r2"] {
            assert!(Gamepads::is_known_axis(name), "{name} is documented");
        }
        assert!(
            !Gamepads::is_known_button("a"),
            "lettered names are not used"
        );
    }
}
