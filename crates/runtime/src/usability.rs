//! Driving a GUI app the way a person would, so a run can tell whether the app
//! is *usable* and not merely valid.
//!
//! Every other gate asks whether an app compiles, imports only `krate:*`, runs,
//! and paints a frame. All four can pass on an app whose buttons do nothing,
//! whose layout ignores the window size, and which closes itself while someone
//! is reading it. Those are the properties a person actually experiences, and
//! nothing measured them until this module.
//!
//! The driver rides along inside a normal headless run. At chosen moments in
//! the app's own loop it grows the window and reads back the app's own canvas
//! rectangle, routes a pointer press and paints the frame either side of it,
//! then simply watches that the window is still open. What it writes out is a
//! set of observations -- not verdicts. Deciding what a difference *means*
//! belongs to `check-app`, which knows about severity and about not crying
//! wolf.
//!
//! Two limits are worth stating rather than hiding, because they bound what a
//! green result means:
//!
//! - **Resize measures the canvas, not the picture.** Nothing in a painted
//!   frame distinguishes a layout that re-flowed from one that was stretched.
//!   So the check turns on whether the app's own canvas followed the window at
//!   all -- a canvas nailed to compile-time constants fails, and an app that
//!   resizes its canvas but still hit-tests against constants passes here.
//! - **A press on a canvas app is a guess.** A canvas app draws its own
//!   buttons, so the host does not know where any of them are and presses the
//!   middle. Landing on empty space is indistinguishable from a dead button,
//!   so that case is reported as unobserved. Only a press on a control the
//!   host itself laid out can produce a failure.
//!
//! Two rules shape everything here:
//!
//! 1. **Observe, never require.** The driver reports what it saw. An
//!    observation it could not make is recorded as "not observed", never as a
//!    failure. A gate that fails an app it merely failed to measure gets
//!    skipped, and a skipped gate protects nothing.
//! 2. **Ride the app's own loop.** Every action happens where the guest asked
//!    for an event, so the app is between frames and in a state it chose. The
//!    driver never reaches into guest memory or forces a redraw the app did not
//!    ask for.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// How long the driver lets an app keep running before it stops watching.
///
/// This is the stay-open observation window, and it has to clear the real bug
/// it was built to catch with room to spare: apps that closed themselves after
/// `MAX_IDLE_ROUNDS = 300` at 33 ms, which is 9.9 seconds of *idle*. The
/// driver's own resize and press reset that idle count, so the self-close
/// lands a second or two later than 9.9 s -- measured at 11.7 s on the
/// reverted fixture.
///
/// Fifteen seconds sits clear of that. The margin matters in one direction
/// only: too short and a healthy app gets cut off mid-watch and reported as
/// one that closed itself, which is the false failure this whole stage must
/// not produce. Too long only costs a few seconds on a run that was going to
/// pass anyway.
pub const STAY_OPEN_WATCH: std::time::Duration = std::time::Duration::from_millis(15_000);

/// The fallback size the driver resizes a window to, when it cannot read the
/// window's current size to grow it instead.
///
/// Normally the driver grows the app's *own* window by a fixed amount, so the
/// resize is a real change whatever size the app chose for itself. Both
/// dimensions always change, so an app that reads only one of width/height
/// still shows a difference.
pub const SECOND_SIZE: (u32, u32) = (900, 620);

/// What the driver is asked to do on a run, and where to put what it saw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsabilityPlan {
    /// Where to write the observations as JSON.
    pub report_path: PathBuf,
    /// Whether to drive a resize and compare frames across it.
    pub check_resize: bool,
    /// Whether to inject a pointer press and compare frames across it.
    pub check_click: bool,
    /// Whether to focus the app's first control and press Enter, then compare
    /// frames across it (IC-743, test 1548). A control that answers the
    /// pointer and not the keyboard is unreachable to anyone without a mouse.
    pub check_keyboard: bool,
    /// Whether to watch for the app closing itself.
    pub check_stay_open: bool,
    /// The app's primary task, scripted, to be completed after the other
    /// checks and before the watch (IC-743, test 1506). None means no task
    /// oracle: reaching readiness is then all that is judged.
    pub task: Option<Task>,
}

/// An app's primary task as steps a driver can do and a state it can look
/// for (IC-743, test 1506): "a GUI that reaches readiness but cannot
/// complete its primary task fails the task oracle". Written by whoever
/// made the app -- a person, or the agent that authored it -- as
/// `krate-check.toml` beside the manifest:
///
/// ```toml
/// name = "add an item to the list"
/// [[step]]
/// click = "Add"
/// [[step]]
/// type = "milk"
/// [[step]]
/// key = "Enter"
/// [[step]]
/// expect_text = "milk"
/// ```
///
/// Clicks find a control by the label the host shows; text and keys go to
/// whatever has focus, the way a person's would; an expectation looks at
/// the labels on screen after the steps before it have settled. An app that
/// draws its own interface (a game) is driven by `click_at` a point of the
/// window and `hold` a key for a time, and `shoot` keeps a picture of what
/// a step did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Task {
    pub name: String,
    #[serde(rename = "step", default)]
    pub steps: Vec<TaskStep>,
}

/// One step. Exactly one field is set; `validate` says so.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct TaskStep {
    /// Press the control whose label is this (case-insensitive, trimmed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub click: Option<String>,
    /// Press the window at a point, "x,y" as fractions of its width and
    /// height ("0.5,0.5" is the middle): for what an app draws itself -- a
    /// game's menu, a canvas -- where there is no labelled control to find.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub click_at: Option<String>,
    /// Hold a key down for a while, "key milliseconds" ("w 3000"): walking
    /// in a game, scrolling with an arrow key. Released when the time is up.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hold: Option<String>,
    /// Hold a gamepad button for a while, "button milliseconds" ("south
    /// 500"), on a stand-in controller the run plugs in (IC-936).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pad: Option<String>,
    /// Hold a stick or trigger at a position for a while, "axis value
    /// milliseconds" ("left-y 1.0 2000"), on the stand-in controller.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stick: Option<String>,
    /// Move a held pointer by "dx dy milliseconds" ("400 0 1000" turns a
    /// camera right): raw motion, as a person's mouse gives a game that holds
    /// the pointer (IC-937).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub look: Option<String>,
    /// Write the window as it is now to this PNG: the evidence of a step
    /// that only a picture shows (a game character that walked). Looks, does
    /// not act.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shoot: Option<String>,
    /// Press and release one key, by the runtime's key name ("Enter", "a"),
    /// with `Shift+` and `Primary+` in front for a chord: "Shift+ArrowLeft"
    /// selects, "Primary+a" is Cmd+A on a Mac and Ctrl+A elsewhere.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// Paste this text the way a person does: it is put on the run's own
    /// clipboard and Cmd+V (Ctrl+V elsewhere) is pressed (K-1109).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paste: Option<String>,
    /// Type this text, one key per character, into whatever has focus.
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub type_text: Option<String>,
    /// Let the app work for this long before the next step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait_ms: Option<u64>,
    /// A label containing this must be on screen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expect_text: Option<String>,
    /// No label containing this may be on screen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expect_no_text: Option<String>,
}

impl TaskStep {
    /// The one thing this step does, or why it is not one thing.
    pub fn validate(&self) -> Result<(), String> {
        let set = [
            self.click.is_some(),
            self.click_at.is_some(),
            self.hold.is_some(),
            self.pad.is_some(),
            self.stick.is_some(),
            self.look.is_some(),
            self.shoot.is_some(),
            self.key.is_some(),
            self.paste.is_some(),
            self.type_text.is_some(),
            self.wait_ms.is_some(),
            self.expect_text.is_some(),
            self.expect_no_text.is_some(),
        ]
        .iter()
        .filter(|s| **s)
        .count();
        if let Some(Err(why)) = self.click_point() {
            return Err(why);
        }
        if let Some(Err(why)) = self.held_key() {
            return Err(why);
        }
        if let Some(Err(why)) = self.pad_press() {
            return Err(why);
        }
        if let Some(Err(why)) = self.stick_tilt() {
            return Err(why);
        }
        if let Some(Err(why)) = self.look_motion() {
            return Err(why);
        }
        if let Some(path) = &self.shoot {
            if !path.to_lowercase().ends_with(".png") {
                return Err(format!("shoot {path:?} must name a .png file"));
            }
        }
        match set {
            1 => Ok(()),
            0 => Err("a step must do one thing: click, click_at, hold, pad, stick, look, key, paste, type, wait_ms, shoot, expect_text or expect_no_text".to_string()),
            _ => Err(format!("a step does one thing, this one does {set}: {}", self.describe())),
        }
    }

    pub fn describe(&self) -> String {
        if let Some(l) = &self.click {
            return format!("click {l:?}");
        }
        if let Some(at) = &self.click_at {
            return format!("click at {at:?}");
        }
        if let Some(h) = &self.hold {
            return format!("hold {h:?}");
        }
        if let Some(p) = &self.shoot {
            return format!("shoot {p:?}");
        }
        if let Some(p) = &self.pad {
            return format!("pad {p:?}");
        }
        if let Some(s) = &self.stick {
            return format!("stick {s:?}");
        }
        if let Some(l) = &self.look {
            return format!("look {l:?}");
        }
        if let Some(k) = &self.key {
            return format!("key {k:?}");
        }
        if let Some(t) = &self.paste {
            return format!("paste {t:?}");
        }
        if let Some(t) = &self.type_text {
            return format!("type {t:?}");
        }
        if let Some(ms) = self.wait_ms {
            return format!("wait {ms} ms");
        }
        if let Some(t) = &self.expect_text {
            return format!("expect {t:?} on screen");
        }
        if let Some(t) = &self.expect_no_text {
            return format!("expect {t:?} not on screen");
        }
        "an empty step".to_string()
    }

    /// A `key` step as (key, shift, primary): "Shift+Primary+a" is
    /// ("a", true, true).
    pub fn key_chord(&self) -> Option<(String, bool, bool)> {
        let mut rest = self.key.as_deref()?;
        let (mut shift, mut primary) = (false, false);
        loop {
            if let Some(after) = rest.strip_prefix("Shift+") {
                shift = true;
                rest = after;
            } else if let Some(after) = rest.strip_prefix("Primary+") {
                primary = true;
                rest = after;
            } else {
                break;
            }
        }
        Some((rest.to_string(), shift, primary))
    }

    /// Where `click_at` presses, as fractions of the window, or why it cannot.
    pub fn click_point(&self) -> Option<Result<(f32, f32), String>> {
        let text = self.click_at.as_ref()?;
        let bad = || {
            format!(
                "click_at {text:?} is not \"x,y\" with each between 0 and 1 (\"0.5,0.5\" is the middle)"
            )
        };
        let mut parts = text.split(',').map(|p| p.trim().parse::<f32>());
        Some(match (parts.next(), parts.next(), parts.next()) {
            (Some(Ok(x)), Some(Ok(y)), None)
                if (0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y) =>
            {
                Ok((x, y))
            }
            _ => Err(bad()),
        })
    }

    /// The key `hold` presses and for how long, or why it cannot.
    pub fn held_key(&self) -> Option<Result<(String, std::time::Duration), String>> {
        let text = self.hold.as_ref()?;
        let mut parts = text.split_whitespace();
        Some(
            match (
                parts.next(),
                parts.next().map(str::parse::<u64>),
                parts.next(),
            ) {
                (Some(key), Some(Ok(ms)), None) if ms > 0 && ms <= 600_000 => {
                    Ok((key.to_string(), std::time::Duration::from_millis(ms)))
                }
                _ => Err(format!(
                    "hold {text:?} is not \"key milliseconds\" (\"w 3000\"), at most ten minutes"
                )),
            },
        )
    }

    /// The pad button a `pad` step holds and for how long, or why it cannot.
    pub fn pad_press(&self) -> Option<Result<(String, std::time::Duration), String>> {
        let text = self.pad.as_ref()?;
        let mut parts = text.split_whitespace();
        Some(match (parts.next(), parts.next().map(str::parse::<u64>), parts.next()) {
            (Some(b), Some(Ok(ms)), None)
                if crate::gamepad::Gamepads::is_known_button(b) && ms > 0 && ms <= 600_000 =>
            {
                Ok((b.to_string(), std::time::Duration::from_millis(ms)))
            }
            _ => Err(format!(
                "pad {text:?} is not \"button milliseconds\" with a krate:ui gamepad button (south, east, west, north, l1, r1, l2, r2, start, select, dpad-up, dpad-down, dpad-left, dpad-right)"
            )),
        })
    }

    /// The axis a `stick` step holds, where, and for how long.
    pub fn stick_tilt(&self) -> Option<Result<(String, f32, std::time::Duration), String>> {
        let text = self.stick.as_ref()?;
        let mut parts = text.split_whitespace();
        Some(
            match (
                parts.next(),
                parts.next().map(str::parse::<f32>),
                parts.next().map(str::parse::<u64>),
                parts.next(),
            ) {
                (Some(a), Some(Ok(v)), Some(Ok(ms)), None)
                    if crate::gamepad::Gamepads::is_known_axis(a)
                        && (-1.0..=1.0).contains(&v)
                        && ms > 0
                        && ms <= 600_000 =>
                {
                    Ok((a.to_string(), v, std::time::Duration::from_millis(ms)))
                }
                _ => Err(format!(
                    "stick {text:?} is not \"axis value milliseconds\" with an axis (left-x, left-y, right-x, right-y, l2, r2) and a value from -1 to 1"
                )),
            },
        )
    }

    /// The motion a `look` step gives and over how long.
    pub fn look_motion(&self) -> Option<Result<(f32, f32, std::time::Duration), String>> {
        let text = self.look.as_ref()?;
        let mut parts = text.split_whitespace();
        Some(
            match (
                parts.next().map(str::parse::<f32>),
                parts.next().map(str::parse::<f32>),
                parts.next().map(str::parse::<u64>),
                parts.next(),
            ) {
                (Some(Ok(dx)), Some(Ok(dy)), Some(Ok(ms)), None)
                    if dx.is_finite() && dy.is_finite() && ms > 0 && ms <= 600_000 =>
                {
                    Ok((dx, dy, std::time::Duration::from_millis(ms)))
                }
                _ => Err(format!(
                    "look {text:?} is not \"dx dy milliseconds\" (\"400 0 1000\")"
                )),
            },
        )
    }

    /// Whether the step changes the app (and so needs a settle after it)
    /// or only looks.
    pub fn is_action(&self) -> bool {
        self.expect_text.is_none() && self.expect_no_text.is_none() && self.shoot.is_none()
    }
}

impl Task {
    /// How long the script itself takes at most: every wait and held key,
    /// and `settle` for each other action (what the driver allows one).
    pub fn scripted_time(&self, settle: std::time::Duration) -> std::time::Duration {
        self.steps
            .iter()
            .map(|s| {
                if let Some(ms) = s.wait_ms {
                    std::time::Duration::from_millis(ms)
                } else if let Some(Ok((_, held))) = s.held_key() {
                    held.max(settle)
                } else if let Some(Ok((_, held))) = s.pad_press() {
                    held.max(settle)
                } else if let Some(Ok((_, _, held))) = s.stick_tilt() {
                    held.max(settle)
                } else if let Some(Ok((_, _, held))) = s.look_motion() {
                    held.max(settle)
                } else if s.is_action() {
                    settle
                } else {
                    std::time::Duration::ZERO
                }
            })
            .sum()
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err(
                "a task needs a name: what a person would call what they are doing".to_string(),
            );
        }
        if self.steps.is_empty() {
            return Err(format!("task {:?} has no steps", self.name));
        }
        if !self.steps.iter().any(|s| !s.is_action()) {
            return Err(format!(
                "task {:?} never looks at the screen; add an expect_text so completing it means something",
                self.name
            ));
        }
        for (index, step) in self.steps.iter().enumerate() {
            step.validate()
                .map_err(|why| format!("task {:?}, step {}: {why}", self.name, index + 1))?;
        }
        Ok(())
    }
}

/// One thing the driver tried to observe.
///
/// `Unobserved` is a first-class outcome and carries why. An app that never
/// opened a window, or never drew anything the driver could compare, produces
/// `Unobserved` -- which `check-app` reports as a note, never as a failure.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "outcome", rename_all = "kebab-case")]
pub enum Observation {
    /// The property held.
    Held,
    /// The property did not hold, with what was seen.
    Broke { detail: String },
    /// Nothing could be measured, with why.
    Unobserved { reason: String },
}

impl Observation {
    pub fn unobserved(reason: impl Into<String>) -> Self {
        Observation::Unobserved {
            reason: reason.into(),
        }
    }

    pub fn broke(detail: impl Into<String>) -> Self {
        Observation::Broke {
            detail: detail.into(),
        }
    }
}

/// Everything one driven run saw. Written as JSON for `check-app` to read.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UsabilityReport {
    /// Did the app keep its window open for the whole watch window.
    pub stay_open: Option<Observation>,
    /// Did the frame change when the window was resized.
    pub resize: Option<Observation>,
    /// Did anything observable change when a pointer press was delivered.
    pub click: Option<Observation>,
    /// Did anything observable change when the first control was focused and
    /// Enter was pressed. Absent in reports written before this existed.
    #[serde(default)]
    pub keyboard: Option<Observation>,
    /// The primary task, when one was given (IC-743, test 1506).
    #[serde(default)]
    pub task: Option<Observation>,
    #[serde(default)]
    pub task_name: Option<String>,
    #[serde(default)]
    pub task_steps_done: usize,
    /// Whether the app ever opened a window at all. A CLI app never does, and
    /// that is not a defect -- it is the signal to skip the whole stage.
    pub opened_window: bool,
    /// How long the app ran before it returned, in milliseconds.
    pub ran_millis: u64,
    /// How the app's canvas lays itself out, for the fit check. Absent for
    /// an app with no canvas (the widget tree always fills its window).
    #[serde(default)]
    pub fit: Option<FitFacts>,
}

/// What decides how an app fills screens of other shapes.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq)]
pub struct FitFacts {
    /// The fixed size the app draws in: the design size it set, or -- when
    /// it never asked its real size -- the size it asked its window to be.
    /// None for an app that lays out from the real canvas size.
    pub fixed: Option<(f32, f32)>,
}

/// The screens an app made once must fill: a phone (held the way the app's
/// shape suggests), a tablet, a desktop window.
pub const FIT_SCREENS: [(&str, f32, f32); 3] = [
    ("a phone", 390.0, 844.0),
    ("a tablet", 820.0, 1180.0),
    ("a desktop window", 1280.0, 800.0),
];

/// A fixed layout fails a screen it must shrink below this much of its
/// designed size to fit -- its text and buttons become too small to read
/// and hit (a 1180x760 desktop layout on a phone shrinks to half) -- or
/// leaves mostly empty, filling less than this much of it.
pub const FIT_MIN_SCALE: f32 = 0.75;
pub const FIT_MIN_FILL: f32 = 0.60;

impl FitFacts {
    /// For each screen, the scale a fixed layout is drawn at and how much of
    /// the screen it fills, scaled uniformly to fit and centred (wide layouts
    /// on a phone or tablet held sideways, as Krate turns the screen for
    /// them). Empty for a layout that fills any screen.
    pub fn fill(&self) -> Vec<(&'static str, f32, f32)> {
        let Some((dw, dh)) = self.fixed else {
            return Vec::new();
        };
        if dw <= 0.0 || dh <= 0.0 {
            return Vec::new();
        }
        FIT_SCREENS
            .iter()
            .map(|&(name, w, h)| {
                let (w, h) = if dw > dh && name != "a desktop window" {
                    (w.max(h), w.min(h))
                } else {
                    (w, h)
                };
                let k = (w / dw).min(h / dh);
                (name, k, (dw * k * dh * k) / (w * h))
            })
            .collect()
    }

    /// The screens this layout fails, as "on a phone it shrinks to 51% of
    /// its size, too small to read" -- or None when it fits them all.
    pub fn wasted(&self) -> Option<String> {
        let findings: Vec<String> = self
            .fill()
            .into_iter()
            .filter_map(|(name, k, f)| {
                if k < FIT_MIN_SCALE {
                    Some(format!(
                        "on {name} it shrinks to {:.0}% of its size, too small to read",
                        k * 100.0
                    ))
                } else if f < FIT_MIN_FILL {
                    Some(format!(
                        "on {name} it fills only {:.0}% of the screen",
                        f * 100.0
                    ))
                } else {
                    None
                }
            })
            .collect();
        (!findings.is_empty()).then(|| findings.join("; "))
    }
}

impl UsabilityReport {
    pub fn write(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
        std::fs::write(path, json)
    }

    pub fn read(path: &Path) -> std::io::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        serde_json::from_str(&text)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))
    }
}

/// Compare two painted frames.
///
/// Returns the fraction of pixels that differ, 0.0 to 1.0. Frames of different
/// dimensions count as fully different, which is the honest answer: the window
/// changed shape, so every pixel moved.
///
/// Whole-pixel equality rather than a perceptual metric, because the question
/// is "did the app draw something different", not "does it look different to a
/// human". Both frames come from the same painter on the same machine in the
/// same process, so there is no encoder noise to tolerate.
pub fn frame_difference(a: &FrameBuffer, b: &FrameBuffer) -> f32 {
    if a.width != b.width || a.height != b.height {
        return 1.0;
    }
    if a.pixels.is_empty() {
        return 0.0;
    }
    let differing = a
        .pixels
        .iter()
        .zip(b.pixels.iter())
        .filter(|(x, y)| x != y)
        .count();
    differing as f32 / a.pixels.len() as f32
}

/// How far a press must beat the app's own animation to count as a reaction.
///
/// A press that changes exactly as much of the screen as a spinner does has
/// proved nothing. Three times clears a spinner, a clock tick, or a blinking
/// cursor, while a real reaction -- a card repainting, a list changing, a
/// toggle flipping -- moves far more of the frame than that.
pub const IDLE_CHURN_MARGIN: f32 = 3.0;

/// Whether a press visibly answered, given how much the app moves on its own.
///
/// The check used to be `difference > 0.0`, which handed the verdict to any
/// animation on screen: one pixel of spinner said "it responded" no matter
/// what was pressed, so an app whose buttons were entirely unreachable passed
/// on the strength of its own loading indicator (K-140, found when both
/// generated weather apps passed every check and neither could be clicked).
pub fn press_answered(difference: f32, idle_churn: f32) -> bool {
    difference > (idle_churn * IDLE_CHURN_MARGIN).max(f32::EPSILON)
}

/// A painted frame held in memory, so frames can be compared without going
/// through PNG encode and decode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameBuffer {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u32>,
}

impl FrameBuffer {
    pub fn new(width: u32, height: u32, pixels: Vec<u32>) -> Self {
        Self {
            width,
            height,
            pixels,
        }
    }

    /// Whether this frame has any content at all -- more than one distinct
    /// colour. A single flat colour means the app cleared its canvas and drew
    /// nothing, which no comparison can say anything useful about.
    /// Whether anything is drawn in the region that only exists after a
    /// resize -- the right and bottom margins beyond the old size.
    ///
    /// This is the question the resize check was missing. An app whose
    /// canvas grows but whose drawing is pinned to constants leaves that
    /// new region empty, which is exactly what a person sees as "the game
    /// is off the screen" after resizing the window.
    pub fn content_in_margin(&self, old_width: u32, old_height: u32) -> bool {
        if self.width <= old_width && self.height <= old_height {
            return false;
        }
        // The background is the most common colour in the NEW region --
        // taking the top-left corner instead was wrong, because in the very
        // case this exists to catch (a layout pinned to the old size) that
        // corner holds painted content while the margin holds background.
        // A margin that is entirely one colour is an unpainted margin.
        let mut margin: Vec<u32> = Vec::new();
        for y in 0..self.height {
            for x in 0..self.width {
                if x < old_width && y < old_height {
                    continue;
                }
                if let Some(p) = self
                    .pixels
                    .get((y as usize) * (self.width as usize) + x as usize)
                {
                    margin.push(*p);
                }
            }
        }
        let Some(&first) = margin.first() else {
            return false;
        };
        margin.iter().any(|p| *p != first)
    }

    pub fn has_content(&self) -> bool {
        let mut iter = self.pixels.iter();
        let Some(first) = iter.next() else {
            return false;
        };
        iter.any(|p| p != first)
    }
}

#[cfg(test)]
mod fit_tests {
    use super::*;

    /// A fixed portrait card (the calculator) wastes every screen; a game
    /// whose proportions are its design fills them; a layout from the real
    /// size fills anything.
    #[test]
    fn the_fit_check_tells_a_fixed_card_from_a_layout_that_fills() {
        // A small fixed card (the calculator) leaves a desktop mostly empty.
        let card = FitFacts {
            fixed: Some((320.0, 470.0)),
        };
        assert_eq!(
            card.wasted().as_deref(),
            Some("on a desktop window it fills only 43% of the screen")
        );
        // A desktop-shaped layout shrinks to half on a phone.
        let desktop = FitFacts {
            fixed: Some((1180.0, 760.0)),
        };
        let wasted = desktop.wasted().expect("unreadable on a phone");
        assert!(
            wasted.starts_with("on a phone it shrinks to 51%"),
            "{wasted}"
        );
        // A 4:3 game is scaled up and fills enough everywhere.
        let game = FitFacts {
            fixed: Some((320.0, 240.0)),
        };
        assert_eq!(game.wasted(), None, "{:?}", game.fill());

        let responsive = FitFacts { fixed: None };
        assert_eq!(responsive.wasted(), None);
        assert!(responsive.fill().is_empty());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// K-096's lock: an app pinned to fixed numbers leaves the new space
    /// empty after a resize, and that is exactly what the check must see.
    #[test]
    fn a_hardcoded_layout_leaves_the_new_margin_empty() {
        // 8x8 frame; the app only ever painted the top-left 4x4.
        let mut pixels = vec![0x00_00_00_00u32; 64];
        for y in 0..4 {
            for x in 0..4 {
                pixels[y * 8 + x] = 0xFF_FF_FF_FF;
            }
        }
        let frame = FrameBuffer {
            width: 8,
            height: 8,
            pixels,
        };
        // Grown from 4x4: nothing new was painted, so this must be false.
        assert!(
            !frame.content_in_margin(4, 4),
            "a layout pinned to 4x4 paints nothing in the margin"
        );

        // An app that follows the window paints across the whole frame.
        let responsive = FrameBuffer {
            width: 8,
            height: 8,
            pixels: (0..64)
                .map(|i| {
                    if i % 2 == 0 {
                        0xFF_00_00_00
                    } else {
                        0xFF_FF_FF_FF
                    }
                })
                .collect(),
        };
        assert!(
            responsive.content_in_margin(4, 4),
            "a responsive layout paints into the space that appeared"
        );
    }

    fn frame(width: u32, height: u32, fill: u32) -> FrameBuffer {
        FrameBuffer::new(width, height, vec![fill; (width * height) as usize])
    }

    #[test]
    fn identical_frames_do_not_differ() {
        let a = frame(4, 4, 0xFF00_00FF);
        let b = frame(4, 4, 0xFF00_00FF);
        assert_eq!(frame_difference(&a, &b), 0.0);
    }

    #[test]
    fn a_different_size_counts_as_wholly_different() {
        // The window changed shape, so every pixel moved. Reporting anything
        // less would let a stretched frame look like a re-laid-out one.
        let a = frame(4, 4, 0);
        let b = frame(8, 8, 0);
        assert_eq!(frame_difference(&a, &b), 1.0);
    }

    #[test]
    fn half_the_pixels_changed_reads_as_half() {
        let a = frame(2, 2, 0);
        let b = FrameBuffer::new(2, 2, vec![0, 0, 1, 1]);
        assert!((frame_difference(&a, &b) - 0.5).abs() < f32::EPSILON);
    }

    /// The hole that let two unclickable apps pass every check (K-140).
    #[test]
    fn a_spinner_cannot_answer_for_a_dead_button() {
        // A weather app's loading spinner, measured: 0.007% of the frame.
        let spinner = 0.000_068;

        // A press that moved no more than the spinner proves nothing, however
        // many pixels that is in absolute terms.
        assert!(
            !press_answered(spinner, spinner),
            "a press that changed exactly what the animation changes must not pass"
        );
        assert!(
            !press_answered(spinner * 2.0, spinner),
            "twice the idle churn is still inside the animation's noise"
        );

        // A real reaction -- the measured city-chip press was 2.86% -- clears
        // it by a wide margin.
        assert!(
            press_answered(0.028_558, spinner),
            "a press that repainted a card must read as a reaction"
        );
    }

    /// A still app is the easy case, and it must not regress into passing a
    /// press that did nothing at all.
    #[test]
    fn a_still_app_needs_any_real_change_to_pass() {
        assert!(!press_answered(0.0, 0.0), "nothing changed is not a pass");
        assert!(press_answered(0.01, 0.0), "a visible change is a pass");
    }

    #[test]
    fn a_flat_frame_has_no_content() {
        assert!(!frame(4, 4, 0xFF12_3456).has_content());
    }

    #[test]
    fn a_frame_with_two_colours_has_content() {
        let mut f = frame(4, 4, 0);
        f.pixels[5] = 0xFFFF_FFFF;
        assert!(f.has_content());
    }

    #[test]
    fn an_empty_frame_has_no_content() {
        assert!(!FrameBuffer::new(0, 0, Vec::new()).has_content());
    }

    #[test]
    fn an_unobserved_check_is_not_a_broken_one() {
        // The rule the whole stage rests on: something the driver could not
        // measure must never read as a defect. If this ever collapses, the
        // stage starts failing good apps, gets skipped, and protects nothing.
        let unobserved = Observation::unobserved("no canvas");
        assert!(
            !matches!(unobserved, Observation::Broke { .. }),
            "an unobserved check must never be a broken one"
        );
    }

    #[test]
    fn a_default_report_claims_nothing() {
        // A run that never got started must not look like a passing one.
        let report = UsabilityReport::default();
        assert!(report.stay_open.is_none());
        assert!(report.resize.is_none());
        assert!(report.click.is_none());
        assert!(
            !report.opened_window,
            "nothing may claim a window until one was seen"
        );
    }

    #[test]
    fn a_report_survives_a_round_trip() {
        let dir = std::env::temp_dir().join(format!("krate-usability-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let path = dir.join("report.json");
        let report = UsabilityReport {
            stay_open: Some(Observation::Held),
            resize: Some(Observation::broke("the frame did not change")),
            keyboard: None,
            task: None,
            task_name: None,
            task_steps_done: 0,
            click: Some(Observation::unobserved("no clickable widget")),
            opened_window: true,
            ran_millis: 1234,
            fit: Some(FitFacts {
                fixed: Some((320.0, 470.0)),
            }),
        };
        report.write(&path).expect("write");
        let back = UsabilityReport::read(&path).expect("read");
        assert_eq!(back.stay_open, Some(Observation::Held));
        assert_eq!(back.fit, report.fit);
        assert_eq!(
            back.resize,
            Some(Observation::broke("the frame did not change"))
        );
        assert!(back.opened_window);
        assert_eq!(back.ran_millis, 1234);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
