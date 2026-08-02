//! Gamepad state behind the `ui.events` gamepad calls.
//!
//! Reading a gamepad on Linux means `libudev`, a system library, and adding one
//! is the sequence that broke two releases: the file picker needed Wayland
//! headers in fifteen CI jobs, speech-to-text needed a newer libclang in a
//! container nobody could see. So the udev headers went into every Linux build
//! and the cross container first, on their own, and this backend followed.
//!
//! Every query answers as though nothing is plugged in when no pad is present,
//! which is what an app must already handle: a person without a controller is
//! the common case, not an error.

use std::collections::BTreeMap;

/// Buttons every controller has, whatever is printed on them.
///
/// Positional names because the same physical button is A on an Xbox pad and B
/// on a Nintendo one; an app asking for `south` gets the button under the
/// player's thumb on both. `gilrs` names them the same way for the same
/// reason, so the mapping below is a rename rather than a reinterpretation.
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
pub struct Gamepads {
    /// The `gilrs` context, or `None` when it could not start.
    ///
    /// A missing or broken input system is not fatal: it means no gamepad,
    /// which is a state every app already handles. A person whose udev is
    /// unusual should get a keyboard-controlled app, not a crash.
    backend: Option<gilrs::Gilrs>,
    /// Buttons currently down, by the portable name.
    held: BTreeMap<String, bool>,
    /// Axis positions, by the portable name.
    axes: BTreeMap<String, f32>,
    /// Whether the last poll saw a connected pad.
    connected: bool,
}

impl std::fmt::Debug for Gamepads {
    /// Hand-written because `gilrs::Gilrs` is not `Debug`, and the useful
    /// summary is what apps can see rather than the backend's internals.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Gamepads")
            .field("backend", &self.backend.is_some())
            .field("connected", &self.connected)
            .field("held", &self.held)
            .field("axes", &self.axes)
            .finish()
    }
}

/// The `gilrs` button behind each portable name.
fn button_of(name: &str) -> Option<gilrs::Button> {
    use gilrs::Button;
    Some(match name {
        "south" => Button::South,
        "east" => Button::East,
        "west" => Button::West,
        "north" => Button::North,
        "l1" => Button::LeftTrigger,
        "r1" => Button::RightTrigger,
        "l2" => Button::LeftTrigger2,
        "r2" => Button::RightTrigger2,
        "start" => Button::Start,
        "select" => Button::Select,
        "dpad-up" => Button::DPadUp,
        "dpad-down" => Button::DPadDown,
        "dpad-left" => Button::DPadLeft,
        "dpad-right" => Button::DPadRight,
        _ => return None,
    })
}

/// The `gilrs` axis behind each portable name.
///
/// `LeftZ` and `RightZ` are the analog triggers, which is why `l2` and `r2`
/// appear both here and in the button list: a trigger can be read as a button
/// that is down past a threshold or as a position, and games want both.
fn axis_of(name: &str) -> Option<gilrs::Axis> {
    use gilrs::Axis;
    Some(match name {
        "left-x" => Axis::LeftStickX,
        "left-y" => Axis::LeftStickY,
        "right-x" => Axis::RightStickX,
        "right-y" => Axis::RightStickY,
        "l2" => Axis::LeftZ,
        "r2" => Axis::RightZ,
        _ => return None,
    })
}

impl Default for Gamepads {
    fn default() -> Self {
        Self::new()
    }
}

impl Gamepads {
    pub fn new() -> Self {
        // A failure here is "no gamepad", not an error worth propagating --
        // there is nothing an app could usefully do differently, and the
        // fallback is the keyboard path it already has.
        let backend = match gilrs::Gilrs::new() {
            Ok(backend) => Some(backend),
            Err(error) => {
                tracing::debug!(%error, "no gamepad support on this machine");
                None
            }
        };
        Self {
            backend,
            held: BTreeMap::new(),
            axes: BTreeMap::new(),
            connected: false,
        }
    }

    /// Take whatever the input system has queued and refresh the cached state.
    ///
    /// Called once per query rather than on a timer. `gilrs` requires its event
    /// queue be drained for state to advance, so a guest that never asks about
    /// gamepads costs nothing, and one that asks every frame gets fresh values.
    fn poll(&mut self) {
        let Some(backend) = self.backend.as_mut() else {
            return;
        };
        while backend.next_event().is_some() {
            // The events themselves are not interesting here: this interface is
            // "what is held right now", not "what changed". Draining is what
            // makes the state below current.
        }

        // First connected pad wins. Couch multiplayer needs a player index in
        // the interface, and inventing one now without an app to check it
        // against is how you get an interface nobody can use.
        let Some((_id, pad)) = backend.gamepads().next() else {
            self.connected = false;
            self.held.clear();
            self.axes.clear();
            return;
        };
        self.connected = true;

        for name in BUTTONS {
            if let Some(button) = button_of(name) {
                self.held
                    .insert((*name).to_string(), pad.is_pressed(button));
            }
        }
        for name in AXES {
            if let Some(axis) = axis_of(name) {
                let value = pad.value(axis);
                // Triggers report 0 to 1 rather than -1 to 1, because a trigger
                // has no negative direction and reporting one would mean every
                // app remapping it. Some backends still hand back a resting
                // -1, so clamp rather than trust.
                let value = if *name == "l2" || *name == "r2" {
                    value.clamp(0.0, 1.0)
                } else {
                    value.clamp(-1.0, 1.0)
                };
                self.axes.insert((*name).to_string(), value);
            }
        }
    }

    pub fn connected(&mut self) -> bool {
        self.poll();
        self.connected
    }

    /// Whether a button is held. An unknown name is not held, rather than an
    /// error: an app checking `triangle` on a pad that calls it `north` should
    /// see nothing happen, not fall over.
    ///
    /// The name is checked against the documented list first, so a backend that
    /// starts reporting some pad-specific button cannot make it visible to apps
    /// without that name being added to the contract as well.
    pub fn held(&mut self, button: &str) -> bool {
        if !Self::is_known_button(button) {
            return false;
        }
        self.poll();
        self.held.get(button).copied().unwrap_or(false)
    }

    /// A stick or trigger position. Zero with no gamepad, so an app that never
    /// checks `gamepad-connected` reads centred sticks rather than drift.
    pub fn axis(&mut self, axis: &str) -> f32 {
        if !Self::is_known_axis(axis) {
            return 0.0;
        }
        self.poll();
        self.axes.get(axis).copied().unwrap_or(0.0)
    }

    /// Whether a name is one this interface defines.
    ///
    /// The backend rejects anything outside these lists rather than inventing a
    /// button an app cannot document.
    pub fn is_known_button(name: &str) -> bool {
        BUTTONS.contains(&name)
    }

    pub fn is_known_axis(name: &str) -> bool {
        AXES.contains(&name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_gamepad_reads_as_centred_and_unpressed() {
        // CI has no controller plugged in, and neither do most people. Both
        // must be quiet rather than surprising -- and this is also the state
        // when `gilrs` itself fails to start, which must not be fatal.
        let mut pads = Gamepads::new();
        if pads.connected() {
            // A developer with a controller attached: skip rather than fail,
            // because the assertions below describe the empty case only.
            return;
        }
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
        let mut pads = Gamepads::new();
        assert!(!pads.held("triangle"));
        assert_eq!(pads.axis("throttle"), 0.0);
    }

    #[test]
    fn an_off_contract_name_stays_invisible_even_when_a_backend_reports_it() {
        // The case a machine with no controller cannot reach on its own: some
        // pad reports a device-specific button, and an app starts depending on
        // a name that will not exist on the next controller. Lookups filter by
        // the documented list so that cannot happen quietly.
        let mut pads = Gamepads::new();
        pads.held.insert("triangle".to_string(), true);
        pads.axes.insert("throttle".to_string(), 0.9);
        assert!(
            !pads.held("triangle"),
            "an undocumented button stays hidden"
        );
        assert_eq!(
            pads.axis("throttle"),
            0.0,
            "an undocumented axis reads centred"
        );
    }

    #[test]
    fn every_documented_name_maps_to_something_the_backend_understands() {
        // The lists, the WIT comment and the `gilrs` mapping have to agree. A
        // name documented but unmapped would read as permanently unpressed and
        // look like a broken controller rather than a missing line here.
        for name in BUTTONS {
            assert!(
                button_of(name).is_some(),
                "{name} is documented but maps to no gilrs button"
            );
            assert!(Gamepads::is_known_button(name));
        }
        for name in AXES {
            assert!(
                axis_of(name).is_some(),
                "{name} is documented but maps to no gilrs axis"
            );
            assert!(Gamepads::is_known_axis(name));
        }
        assert!(
            !Gamepads::is_known_button("a"),
            "lettered names are not used"
        );
        assert!(button_of("triangle").is_none());
        assert!(axis_of("throttle").is_none());
    }

    #[test]
    fn no_two_names_map_to_the_same_control() {
        // A copy-paste slip in the match arms above would silently alias two
        // buttons -- pressing one would light up both, which is the kind of bug
        // that gets blamed on the controller.
        let mut seen = Vec::new();
        for name in BUTTONS {
            if let Some(button) = button_of(name) {
                assert!(!seen.contains(&button), "{name} duplicates another button");
                seen.push(button);
            }
        }
        let mut seen = Vec::new();
        for name in AXES {
            if let Some(axis) = axis_of(name) {
                assert!(!seen.contains(&axis), "{name} duplicates another axis");
                seen.push(axis);
            }
        }
    }
}
