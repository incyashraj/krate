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
    /// Whether to watch for the app closing itself.
    pub check_stay_open: bool,
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
    /// Whether the app ever opened a window at all. A CLI app never does, and
    /// that is not a defect -- it is the signal to skip the whole stage.
    pub opened_window: bool,
    /// How long the app ran before it returned, in milliseconds.
    pub ran_millis: u64,
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
            click: Some(Observation::unobserved("no clickable widget")),
            opened_window: true,
            ran_millis: 1234,
        };
        report.write(&path).expect("write");
        let back = UsabilityReport::read(&path).expect("read");
        assert_eq!(back.stay_open, Some(Observation::Held));
        assert_eq!(
            back.resize,
            Some(Observation::broke("the frame did not change"))
        );
        assert!(back.opened_window);
        assert_eq!(back.ran_millis, 1234);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
