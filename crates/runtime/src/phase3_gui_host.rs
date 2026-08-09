//! Host implementation for the Phase 3 `gui` world's new imports.
//!
//! `Phase3GuiHost` backs the `krate:ui` interfaces with the UCap-gated
//! Phase 3 UI dispatcher. Window, widget-tree, and event calls are real;
//! after every tree change the host recomputes layout and re-lowers the
//! supported widgets to native controls when the selected adapter can (the
//! opt-in macOS AppKit prototype today — headless adapters lower nothing and
//! that is a valid state). Audio capture and playback drive real CPAL streams.
//! The `gfx` and `menu` surfaces return honest `unsupported` errors until
//! their runtimes exist.

use krate_adapter_common::painter::drawn_kind;
use krate_adapter_common::ui::{
    kind_is_selectable, ImagePixels, Modifiers, PointerButton, Theme, UiAdapterError, UiEvent,
    WidgetId, WidgetKind, WidgetNode, WidgetPlacement, WidgetStyle, WindowBackendKind, WindowId,
    WindowOptions, WindowSize,
};
use krate_layout::{absolute_rect, LayoutViewport};
use std::sync::Arc;

use crate::{
    audio_capture::{AudioCaptureRuntime, CaptureConfig, CaptureError, CaptureSampleFormat},
    audio_playback::{AudioPlaybackRuntime, PlaybackConfig, PlaybackError, PlaybackSampleFormat},
    canvas_raster::{pack_color, CanvasSurface},
    phase3_gui_bindings::krate::{audio, gfx, speech, ui},
    phase3_ui::{Phase3HostUiMode, Phase3UiDispatcher, Phase3UiRuntime, UiDispatchError},
    scene3d::Scene,
    speech_transcription::{LocalSpeechRuntime, SpeechError},
    uapi::{AudioCall, UapiCall, UapiGuard, UiCall},
};

/// How long `events.wait` sleeps between polls.
const WAIT_POLL_INTERVAL_MILLIS: u64 = 10;

/// How many idle `events.wait` calls a headless run tolerates before the host
/// reports the window closed. Small enough that `krate run` on a GUI app
/// finishes in about a second instead of spinning out the app's wait budget,
/// large enough that an app doing a few no-event waits during start-up still
/// reaches its event loop. Only ever consulted on the headless path.
const HEADLESS_IDLE_WAIT_LIMIT: u32 = 8;

/// How long a headless GUI run may keep waiting for events before the host
/// reports the window closed.
///
/// The idle-wait limit above only counts *unbounded* waits. An animation loop
/// always passes a timeout -- that is how it gets a frame every 16 ms -- so it
/// never reaches that limit and never ends. A person who runs an animated app
/// with no window watches a frozen terminal, which is what happened to the
/// bouncing-ball sample: ten minutes of nothing.
///
/// Wall-clock is the honest bound because it does not care how the guest
/// waits. Five seconds is far longer than any verification run needs and short
/// enough that a person who runs an app by mistake gets their prompt back.
const HEADLESS_RUN_BUDGET: std::time::Duration = std::time::Duration::from_secs(5);

/// Host state for the Phase 3 `gui` world imports.
pub struct Phase3GuiHost {
    runtime: Phase3UiRuntime,
    windows: Vec<WindowId>,
    /// Files the person chose in a dialog this run.
    ///
    /// The picker writes here and `fs.open-chosen` reads, so the two halves of
    /// one grant share a store that lives and dies with the run. An app cannot
    /// carry a token across runs because this is gone when the run ends.
    chosen_files: std::rc::Rc<std::cell::RefCell<crate::chosen_files::ChosenFiles>>,
    /// Host-side vertical scroll offsets per (window, Scroll widget).
    /// Scrolling never involves the guest: wheel input adjusts these and
    /// re-lowers placements, matching native platform feel.
    scroll_offsets: std::cell::RefCell<std::collections::BTreeMap<(WindowId, WidgetId), f32>>,
    /// Last text the host observed in each natively lowered editable control.
    /// AppKit keeps typed characters inside the control, so the guest only
    /// learns about them by the host reading the control back and comparing.
    native_text: std::cell::RefCell<std::collections::BTreeMap<(WindowId, WidgetId), String>>,
    /// True when no window on this host can ever receive human input, i.e. the
    /// headless draft path. A GUI app's normal shape is "loop until the person
    /// closes the window", so with no such window the loop has nothing to end
    /// it and the app spins out its whole wait budget. See [`idle_waits`].
    headless: bool,
    /// Consecutive `wait` calls that timed out with no event, used only when
    /// [`headless`] is set. Once this passes [`HEADLESS_IDLE_WAIT_LIMIT`] the
    /// host synthesises a close request so the guest exits the way it would if
    /// a person had closed the window.
    idle_waits: std::cell::Cell<u32>,
    /// When the last frame was published, for pacing the next one.
    last_present: std::cell::Cell<Option<std::time::Instant>>,
    /// How many times an interrupt has been turned into a close request.
    interrupts: std::cell::Cell<u32>,
    /// How many times the person has asked to close the window.
    ///
    /// The first request goes to the guest untouched. A second means the guest
    /// is not listening, and the runtime closes the window itself rather than
    /// leaving a button that does nothing.
    close_requests: std::cell::Cell<u32>,
    /// Events pumped by `key-held` that the app has not been given yet.
    ///
    /// `key-held` has to pump the platform queue, or a game that never calls
    /// `poll` would read stale input. But pumping returns whatever event came
    /// out, and throwing that away destroyed it: a game reading ten keys a
    /// frame swallowed the window's close request before its own `poll` could
    /// see it, so clicking the close button did nothing. Pumped events wait
    /// here and are handed over by the next `poll` or `wait`.
    pending_events: std::cell::RefCell<std::collections::VecDeque<ui::types::Event>>,
    /// When this headless run began waiting for events, for [`HEADLESS_RUN_BUDGET`].
    /// Set on the first wait rather than at construction, so time spent
    /// building the window tree is not charged against the budget.
    headless_started: std::cell::Cell<Option<std::time::Instant>>,
    /// Pictures for image widgets, keyed by the widget they belong to.
    ///
    /// Held here rather than on the widget node because a picture arrives
    /// through its own interface: adding a field to `widget-node` would change
    /// that record's type and stop every GUI app already built from
    /// instantiating at all.
    images: std::cell::RefCell<std::collections::BTreeMap<(WindowId, WidgetId), Arc<ImagePixels>>>,
    /// Bound 2D canvases, keyed by the id handed to the guest. Each remembers
    /// which widget it publishes to; the pixels land in [`Self::images`] and
    /// travel the image widget's proven path to all three systems.
    canvases:
        std::cell::RefCell<std::collections::BTreeMap<u64, (WindowId, WidgetId, CanvasSurface)>>,
    /// Connected gamepads, polled on demand.
    gamepads: std::cell::RefCell<crate::gamepad::Gamepads>,
    /// Keys currently held down, for `events.key-held`.
    ///
    /// Tracked here rather than left to the app because the host is the only
    /// side that sees a window lose focus. An app tracking presses itself
    /// keeps running forever when someone alt-tabs mid-stride: the release
    /// went to another window and never arrives.
    held_keys: std::cell::RefCell<std::collections::BTreeSet<String>>,
    /// Bound 3D scenes, sharing the canvas id space so a scene and a canvas
    /// can never collide on one number.
    scenes: std::cell::RefCell<std::collections::BTreeMap<u64, (WindowId, WidgetId, Scene)>>,
    /// The next canvas or scene id to hand out; never reused within a run.
    next_canvas_id: std::cell::Cell<u64>,
    /// Native microphone streams owned by this one sandboxed app session.
    audio_capture: AudioCaptureRuntime,
    /// Native speaker streams owned by this one sandboxed app session.
    audio_playback: AudioPlaybackRuntime,
    /// Local speech model contexts, scoped to this one sandboxed app session.
    speech: LocalSpeechRuntime,
    /// A screenshot request: paint the window to this PNG at this scale once
    /// the app has drawn a frame. `taken` guards against writing every frame --
    /// the first drawn frame is the one captured.
    screenshot: Option<(std::path::PathBuf, f32)>,
    screenshot_taken: std::cell::Cell<bool>,
    /// The usability script, when this run is a driven one. `None` on every
    /// ordinary run, which is what keeps this whole mechanism off the path a
    /// person's app takes.
    usability: Option<UsabilityDriver>,
}

/// The scripted half of a driven run: which step is next, and what each one saw.
///
/// Kept beside the host rather than inside it because the driver is a
/// verification harness, not part of what an app can observe. Nothing here is
/// reachable from the guest.
struct UsabilityDriver {
    plan: crate::usability::UsabilityPlan,
    /// Where the script is up to.
    step: DriveStep,
    /// When the driven run started waiting, for the stay-open watch.
    started: Option<std::time::Instant>,
    /// The frame painted just before the pending action, to compare against.
    before: Option<crate::usability::FrameBuffer>,
    /// What the run has seen so far.
    report: crate::usability::UsabilityReport,
    /// Whether the current step has already delivered its action and is now
    /// waiting one turn of the app's loop before comparing frames.
    action_delivered: bool,
    /// Whether the pending press landed on a control the host actually knows
    /// the rectangle of. A canvas app draws its own buttons, so a press there
    /// is an educated guess and no verdict may be built on it.
    press_was_confident: bool,
    /// The app's canvas rectangle before the resize, which is what the resize
    /// check compares against.
    canvas_before: Option<(f32, f32)>,
    /// The size the window was actually grown to, for the failure message.
    resized_to: Option<(u32, u32)>,
    /// When the script last advanced, so a polling game and an event-loop app
    /// are driven at the same pace.
    last_step: Option<std::time::Instant>,
}

/// How long the driver leaves between steps of its script.
///
/// Long enough that the app gets a real turn of its own loop to react to each
/// action before the next observation is taken, short enough that the whole
/// script fits inside the stay-open watch with room to spare.
const STEP_INTERVAL: std::time::Duration = std::time::Duration::from_millis(250);

/// How long the driver waits for an app to draw its first frame before giving
/// up on the comparisons and just watching that the window stays open.
///
/// Bounded so an app that paints nothing cannot hold the run open until the
/// outer verification watchdog kills it -- a killed run would be reported as a
/// defect the app does not have.
const SETTLE_GRACE: std::time::Duration = std::time::Duration::from_millis(3_000);

/// The steps of a driven run, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DriveStep {
    /// Let the app settle and draw its first real frame.
    Settle,
    /// Capture the frame, then resize the window.
    Resize,
    /// Compare the post-resize frame, then deliver a pointer press.
    Click,
    /// Compare the post-click frame, then just watch that it stays open.
    Watch,
    /// The script is finished; end the run.
    Done,
}

/// Set when the person presses Ctrl-C.
///
/// A guest loop does not stop on its own: the signal reaches the process and
/// the wasm keeps running, so three presses did nothing and the window stayed
/// open. Recording it here lets the event paths turn it into the close the
/// app already knows how to handle, which also means an app that saves on the
/// way out still gets to.
pub static INTERRUPTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Install the Ctrl-C handler. Safe to call more than once.
pub fn install_interrupt_handler() {
    #[cfg(unix)]
    unsafe {
        extern "C" fn on_interrupt(_signal: i32) {
            INTERRUPTED.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        libc::signal(libc::SIGINT, on_interrupt as libc::sighandler_t);
    }
}

fn interrupted() -> bool {
    INTERRUPTED.load(std::sync::atomic::Ordering::SeqCst)
}

impl Phase3GuiHost {
    /// Create the GUI host with the requested host UI mode.
    pub fn new(guard: UapiGuard, mode: Phase3HostUiMode) -> Result<Self, UiDispatchError> {
        let runtime = Phase3UiRuntime::try_with_host_adapter_mode(guard, mode)?;
        // Read before `runtime` moves into the struct below.
        let headless_backend =
            runtime.adapter_info().window_backend == WindowBackendKind::HeadlessDraft;
        Ok(Self {
            runtime,
            chosen_files: Default::default(),
            windows: Vec::new(),
            scroll_offsets: std::cell::RefCell::new(std::collections::BTreeMap::new()),
            native_text: std::cell::RefCell::new(std::collections::BTreeMap::new()),
            // Judged by the adapter actually serving this run, not the mode
            // that was requested. NativeWithHeadlessFallback can land on the
            // headless adapter, and deriving this flag from the request meant
            // that run got neither a window nor the headless exit budget: it
            // looped forever, invisible and silent, which is exactly what a
            // person saw on their first `krate run`.
            headless: headless_backend,
            idle_waits: std::cell::Cell::new(0),
            last_present: std::cell::Cell::new(None),
            interrupts: std::cell::Cell::new(0),
            close_requests: std::cell::Cell::new(0),
            pending_events: std::cell::RefCell::new(std::collections::VecDeque::new()),
            headless_started: std::cell::Cell::new(None),
            images: std::cell::RefCell::new(std::collections::BTreeMap::new()),
            canvases: std::cell::RefCell::new(std::collections::BTreeMap::new()),
            gamepads: std::cell::RefCell::new(crate::gamepad::Gamepads::new()),
            held_keys: std::cell::RefCell::new(std::collections::BTreeSet::new()),
            scenes: std::cell::RefCell::new(std::collections::BTreeMap::new()),
            next_canvas_id: std::cell::Cell::new(1),
            audio_capture: AudioCaptureRuntime::default(),
            audio_playback: AudioPlaybackRuntime::default(),
            speech: LocalSpeechRuntime::default(),
            screenshot: None,
            screenshot_taken: std::cell::Cell::new(false),
            usability: None,
        })
    }

    pub fn with_asset_root(mut self, root: Option<std::path::PathBuf>) -> Self {
        self.speech = LocalSpeechRuntime::default().with_asset_root(root);
        self
    }

    /// Drive this run against a usability script instead of letting it idle out.
    ///
    /// Only ever set by `check-app`'s usability stage. An ordinary run passes
    /// `None` and behaves exactly as it always has, which is the point: this is
    /// a harness bolted onto the side of the real run, not a change to it.
    pub fn with_usability(mut self, plan: Option<crate::usability::UsabilityPlan>) -> Self {
        self.usability = plan.map(|plan| UsabilityDriver {
            plan,
            step: DriveStep::Settle,
            started: None,
            before: None,
            report: crate::usability::UsabilityReport::default(),
            action_delivered: false,
            press_was_confident: false,
            canvas_before: None,
            resized_to: None,
            last_step: None,
        });
        self
    }

    /// Ask the host to paint the window to `path` at `scale` once the app has
    /// drawn a frame. Only meaningful on a headless run, where there is no real
    /// window to grab from the window server.
    pub fn with_screenshot(mut self, request: Option<(std::path::PathBuf, f32)>) -> Self {
        self.screenshot = request;
        self
    }

    /// If a screenshot was requested and not yet taken, paint the frame now.
    ///
    /// Called at the top of the app's event wait, which is the first moment the
    /// tree is fully built: an app constructs its whole window, then waits for
    /// input. Capturing mid-construction (on every tree sync) grabbed a
    /// half-built frame -- a checklist with its title but not its rows.
    fn maybe_take_screenshot(&self) {
        let Some(window) = self.windows.first().copied() else {
            return;
        };
        self.maybe_take_screenshot_for(window);
    }

    /// Capture a specific window, used on close so an app that never waits for
    /// input still gets its final frame shot.
    fn maybe_take_screenshot_for(&self, window: WindowId) {
        if self.screenshot_taken.get() || self.screenshot.is_none() {
            return;
        }
        let Some((path, scale)) = self.screenshot.as_ref() else {
            return;
        };
        match self.render_window_png(window, *scale, path) {
            Ok(()) => self.screenshot_taken.set(true),
            // Nothing to render yet is not an error: the very first wait can
            // arrive before the tree exists. A later wait will succeed.
            Err(error) => tracing::debug!(?error, "screenshot not ready yet"),
        }
    }

    fn dispatcher(&self) -> Phase3UiDispatcher<'_> {
        self.runtime.dispatcher()
    }

    /// On a headless host, report a window close once the guest has waited
    /// [`HEADLESS_IDLE_WAIT_LIMIT`] times with nothing to show for it.
    ///
    /// A GUI app is written as "keep going until the window closes", which on a
    /// real desktop ends when the person closes it. Headless has no window and
    /// no person, so an app that waits with no timeout can never be released,
    /// and the run hangs with nothing on screen to explain why.
    ///
    /// This only ever fires for an unbounded wait, where the alternative is
    /// waiting forever. A wait that carries a timeout is left alone to time out
    /// on its own: the guest asked to be given back control after a set time
    /// and it will be, so there is no need to tell it anything happened. That
    /// distinction matters because "the window closed" means a person closed
    /// it, and apps report it as such — `krate-hello-gui` exits 2 for "user
    /// closed the window" versus 1 for "finished without one". Synthesising the
    /// close on every idle timeout made those runs claim a person acted when
    /// nobody had.
    /// Report a window close once a headless run has spent its whole budget.
    ///
    /// Independent of how the guest waits, which is the point: the idle-wait
    /// counter is defeated by any timeout, and an animated app always passes
    /// one.
    /// Tick every native window's event loop once, without blocking.
    ///
    /// The adapters route real input, close clicks and redraws through this,
    /// and it was never called: `events.wait` slept between polls of a queue
    /// nothing native ever fed. A window that existed but took no input and
    /// ignored its close button was the result. Headless runs skip it -- the
    /// draft adapter has no native loop to tick.
    fn pump_native_windows(&self) {
        if self.headless {
            return;
        }
        for id in &self.windows {
            if let Err(error) = self.dispatcher().pump_event_loop_once(*id) {
                tracing::debug!(?error, window = id.get(), "native event pump failed");
            }
        }
    }

    /// Advance the usability script by one step, at the app's own event wait.
    ///
    /// Returns `Some(event)` only to end the run once the script is finished.
    /// Every other step returns `None`, which leaves the app waiting exactly as
    /// it was -- the driver's actions arrive as ordinary queued events the app
    /// picks up on its next poll, so nothing here is a path a real app cannot
    /// take.
    ///
    /// Each step runs one wait apart rather than back to back, so the app has a
    /// turn of its own loop to react to the last action before the next frame
    /// is compared against it.
    fn drive_usability_step(&mut self) -> Option<ui::types::Event> {
        let window = self.windows.first().copied();
        let Some(window) = window else {
            // No window yet. A CLI app never opens one and the stage skips on
            // exactly this signal.
            return None;
        };
        {
            let now = std::time::Instant::now();
            let driver = self.usability.as_mut()?;
            driver.report.opened_window = true;
            if driver.started.is_none() {
                driver.started = Some(now);
            }
            // Pace the script by wall clock, not by how often the app asks for
            // events. An event-loop app calls in about thirty times a second; a
            // game polling at the top of every frame calls in thousands of
            // times a second. Without this the whole script would run out in
            // the first few milliseconds of a game, before it had drawn
            // anything, and every observation would be of a blank window.
            match driver.last_step {
                Some(last) if now.duration_since(last) < STEP_INTERVAL => return None,
                _ => driver.last_step = Some(now),
            }
        }

        let step = self.usability.as_ref()?.step;
        match step {
            DriveStep::Settle => {
                // A little grace so the first fully-drawn frame is the
                // baseline, not a half-built tree.
                let frame = self.capture_frame(window);
                let driver = self.usability.as_mut()?;
                if frame.as_ref().is_some_and(|f| f.has_content()) {
                    driver.before = frame;
                    driver.step = if driver.plan.check_resize {
                        DriveStep::Resize
                    } else if driver.plan.check_click {
                        DriveStep::Click
                    } else {
                        DriveStep::Watch
                    };
                    return None;
                }
                // Nothing drawn yet. Keep waiting, but not forever: an app that
                // opens a window and paints nothing would otherwise hold the
                // run until the outer watchdog killed it, and a killed run
                // reports as a defect the app does not have. Give up on the
                // comparisons and go watch that it at least stays open, which
                // is still worth knowing.
                if driver.started.is_some_and(|s| s.elapsed() >= SETTLE_GRACE) {
                    let reason = "the app opened a window but never drew anything into it, so \
                                  there was no frame to compare";
                    if driver.plan.check_resize {
                        driver.report.resize =
                            Some(crate::usability::Observation::unobserved(reason));
                    }
                    if driver.plan.check_click {
                        driver.report.click =
                            Some(crate::usability::Observation::unobserved(reason));
                    }
                    driver.step = DriveStep::Watch;
                }
                None
            }
            DriveStep::Resize => {
                self.drive_resize(window);
                None
            }
            DriveStep::Click => {
                self.drive_click(window);
                None
            }
            DriveStep::Watch => {
                let driver = self.usability.as_mut()?;
                let started = driver.started?;
                if std::time::Instant::now().duration_since(started)
                    < crate::usability::STAY_OPEN_WATCH
                {
                    return None;
                }
                // The app was still running at the end of the watch, which is
                // the whole question for the stay-open check.
                if driver.plan.check_stay_open {
                    driver.report.stay_open = Some(crate::usability::Observation::Held);
                }
                driver.step = DriveStep::Done;
                Some(ui::types::Event::CloseRequested(window.get()))
            }
            DriveStep::Done => Some(ui::types::Event::CloseRequested(window.get())),
        }
    }

    /// Resize the window, then see whether the app's own canvas followed it.
    ///
    /// Runs across two visits: the first records the canvas rectangle and
    /// queues the resize, the second reads the canvas rectangle back.
    ///
    /// The canvas is the measurement, not the window. Resizing the window
    /// always changes the window -- that is the host obeying itself, and an
    /// app that ignores the whole event still "passes" if you look there. The
    /// canvas is sized by the app's own widget style, so a canvas that stayed
    /// put is an app whose drawing surface is nailed to compile-time
    /// constants: exactly the shape where clicks land in the wrong place after
    /// a resize.
    fn drive_resize(&mut self, window: WindowId) {
        let resized_already = self.usability.as_ref().is_some_and(|d| d.action_delivered);

        if !resized_already {
            let before_rect = self.canvas_rect(window);
            // Grow the app's *own* window rather than jump to a fixed size, so
            // the resize is always a real change no matter what size the app
            // chose. An app that happened to open at the driver's target size
            // would otherwise be asked to resize to the size it already was,
            // and reported as ignoring an event that never happened.
            let current = self
                .dispatcher()
                .window(window)
                .ok()
                .flatten()
                .map(|record| (record.size.width, record.size.height));
            let (target_w, target_h) = match current {
                Some((w, h)) => (w.saturating_add(220), h.saturating_add(140)),
                None => crate::usability::SECOND_SIZE,
            };
            let size = match WindowSize::new(target_w, target_h) {
                Ok(size) => size,
                Err(_) => return,
            };
            if self.dispatcher().set_size(window, size).is_err() {
                if let Some(driver) = self.usability.as_mut() {
                    driver.report.resize = Some(crate::usability::Observation::unobserved(
                        "the window could not be resized on this host",
                    ));
                    driver.step = Self::next_after_resize(driver);
                }
                return;
            }
            // The resize is queued; compare on the next visit, once the app has
            // had a turn of its own loop to react to it.
            if let Some(driver) = self.usability.as_mut() {
                driver.action_delivered = true;
                driver.canvas_before = before_rect;
                driver.resized_to = Some((target_w, target_h));
            }
            return;
        }

        let after_rect = self.canvas_rect(window);
        let after_frame = self.capture_frame(window);
        let driver = match self.usability.as_mut() {
            Some(driver) => driver,
            None => return,
        };
        driver.action_delivered = false;
        driver.report.resize = Some(match (driver.canvas_before, after_rect) {
            (Some(before), Some(after)) => {
                // Compared with a tolerance because these are laid-out floats,
                // and a one-pixel rounding difference is not an app reacting.
                let grew = (after.0 - before.0).abs() > 1.0 || (after.1 - before.1).abs() > 1.0;
                if grew {
                    crate::usability::Observation::Held
                } else {
                    let (target_w, target_h) =
                        driver.resized_to.unwrap_or(crate::usability::SECOND_SIZE);
                    crate::usability::Observation::broke(format!(
                        "the window was resized to {target_w}x{target_h} and the app's canvas \
                         stayed {:.0}x{:.0}, so its layout is not following the window",
                        before.0, before.1
                    ))
                }
            }
            // An app with no canvas draws through ordinary widgets, which the
            // layout engine reflows on its own. There is nothing here that the
            // app could get wrong, so there is nothing to report.
            _ => crate::usability::Observation::unobserved(
                "this app draws with laid-out widgets rather than a canvas, so the layout \
                 engine handles resizing for it",
            ),
        });
        driver.before = after_frame;
        driver.step = Self::next_after_resize(driver);
    }

    fn next_after_resize(driver: &UsabilityDriver) -> DriveStep {
        if driver.plan.check_click {
            DriveStep::Click
        } else {
            DriveStep::Watch
        }
    }

    /// Deliver a pointer press, then judge the next frame against the one
    /// before it. Runs across two visits, like the resize.
    fn drive_click(&mut self, window: WindowId) {
        let clicked_already = self.usability.as_ref().is_some_and(|d| d.action_delivered);
        let before = self.usability.as_ref().and_then(|d| d.before.clone());

        if !clicked_already {
            let Some((x, y, confident)) = self.usability_press_target(window) else {
                if let Some(driver) = self.usability.as_mut() {
                    driver.report.click = Some(crate::usability::Observation::unobserved(
                        "the app drew nothing that could be pressed",
                    ));
                    driver.step = DriveStep::Watch;
                }
                return;
            };
            // Press and release, because an app may act on either. Routed the
            // same way a real click is, so it hit-tests against the tree the
            // app actually built and arrives carrying the widget it landed on.
            let viewport = self
                .window_placements(window)
                .ok()
                .flatten()
                .and_then(|(size, _)| {
                    LayoutViewport::new(size.width as f32, size.height as f32).ok()
                });
            let Some(viewport) = viewport else {
                if let Some(driver) = self.usability.as_mut() {
                    driver.report.click = Some(crate::usability::Observation::unobserved(
                        "the window had no laid-out frame to press into",
                    ));
                    driver.step = DriveStep::Watch;
                }
                return;
            };
            let dispatcher = self.dispatcher();
            let press = |pressed: bool| {
                dispatcher.route_pointer_event(crate::phase3_ui::PointerRouteRequest {
                    window,
                    viewport,
                    x,
                    y,
                    button: Some(krate_adapter_common::ui::PointerButton::Primary),
                    pressed,
                    modifiers: Default::default(),
                })
            };
            let pressed = press(true);
            let released = press(false);
            if pressed.is_err() || released.is_err() {
                if let Some(driver) = self.usability.as_mut() {
                    driver.report.click = Some(crate::usability::Observation::unobserved(
                        "this host could not deliver a pointer press",
                    ));
                    driver.step = DriveStep::Watch;
                }
                return;
            }
            if let Some(driver) = self.usability.as_mut() {
                driver.action_delivered = true;
                driver.press_was_confident = confident;
            }
            return;
        }

        let after = self.capture_frame(window);
        let driver = match self.usability.as_mut() {
            Some(driver) => driver,
            None => return,
        };
        let confident = driver.press_was_confident;
        driver.action_delivered = false;
        driver.report.click = Some(match (before, after.clone()) {
            (Some(before), Some(after)) => {
                let difference = crate::usability::frame_difference(&before, &after);
                if difference > 0.0 {
                    crate::usability::Observation::Held
                } else if confident {
                    // The press landed on a control the host laid out, so the
                    // app was pressed exactly where its own widget is and
                    // nothing at all happened.
                    crate::usability::Observation::broke(
                        "a press on the middle of the app's own clickable control changed \
                         nothing on screen",
                    )
                } else {
                    // A canvas app draws its own controls, so the host has no
                    // idea where they are and pressed the middle of the canvas.
                    // Landing on empty space looks exactly like a dead button,
                    // and guessing wrong must never fail an app.
                    crate::usability::Observation::unobserved(
                        "the frame did not change after a press, but this app draws its own \
                         controls so the press may simply have landed on empty space",
                    )
                }
            }
            _ => crate::usability::Observation::unobserved(
                "no frame could be painted around the press",
            ),
        });
        driver.before = after;
        driver.step = DriveStep::Watch;
    }

    /// Hand the finished report back so the run can write it out.
    ///
    /// This is where the stay-open verdict is really decided. The driver only
    /// ever records `Held` from inside the watch, after the full window has
    /// elapsed. If the run reaches here with the watch unfinished, the app
    /// returned from its own event loop while the driver was still watching --
    /// which is precisely "the window closed by itself".
    pub fn usability_report(&self) -> Option<crate::usability::UsabilityReport> {
        self.usability.as_ref().map(|d| {
            let mut report = d.report.clone();
            let ran = d.started.map(|s| s.elapsed()).unwrap_or_default();
            report.ran_millis = ran.as_millis() as u64;
            if d.plan.check_stay_open && report.stay_open.is_none() && report.opened_window {
                report.stay_open = Some(crate::usability::Observation::broke(format!(
                    "the app opened a window and then closed it by itself after {:.1}s, with \
                     nobody asking it to",
                    ran.as_secs_f32()
                )));
            }
            report
        })
    }

    /// Where the report should be written, if this is a driven run.
    pub fn usability_report_path(&self) -> Option<std::path::PathBuf> {
        self.usability.as_ref().map(|d| d.plan.report_path.clone())
    }

    fn headless_budget_close_request(&self) -> Option<ui::types::Event> {
        if !self.headless {
            return None;
        }
        let now = std::time::Instant::now();
        let started = match self.headless_started.get() {
            Some(started) => started,
            None => {
                self.headless_started.set(Some(now));
                return None;
            }
        };
        if now.duration_since(started) < HEADLESS_RUN_BUDGET {
            return None;
        }
        let window = self.windows.first().map(|id| id.get()).unwrap_or(0);
        Some(ui::types::Event::CloseRequested(window))
    }

    /// Paint the current frame into memory, for comparing one moment to another.
    ///
    /// The same placements and the same painter `render_window_png` uses, so a
    /// comparison here is a comparison of what a person would have seen.
    fn capture_frame(&self, window: WindowId) -> Option<crate::usability::FrameBuffer> {
        let (size, placements) = self.window_placements(window).ok()??;
        let width = (size.width as f32).round().max(1.0) as u32;
        let height = (size.height as f32).round().max(1.0) as u32;
        let mut buffer = vec![0u32; width as usize * height as usize];
        krate_adapter_common::painter::paint_placements(
            &mut buffer,
            width,
            height,
            1.0,
            &placements,
            krate_adapter_common::painter::PaintInteraction::default(),
        );
        Some(crate::usability::FrameBuffer::new(width, height, buffer))
    }

    /// The laid-out rectangle of the app's canvas, if it drew into one.
    ///
    /// This is the number the resize check turns on, and it is deliberately not
    /// the window's size. The window always changes when the host resizes it --
    /// that is the host obeying itself and says nothing about the app. The
    /// canvas is the app's own drawing surface, sized by the app's own widget
    /// style, so a canvas that refuses to follow the window is the app pinning
    /// its layout to compile-time constants.
    fn canvas_rect(&self, window: WindowId) -> Option<(f32, f32)> {
        let (_, placements) = self.window_placements(window).ok()??;
        placements
            .iter()
            .find(|placement| placement.kind == WidgetKind::Canvas)
            .map(|placement| (placement.width, placement.height))
    }

    /// Pick a point that is worth pressing, and say whether it is a real
    /// control or only a guess.
    ///
    /// A canvas app draws its own buttons, so the host sees one big canvas and
    /// cannot know where anything is. Pressing the middle of the canvas is the
    /// best available guess -- and because it is a guess, a canvas app that
    /// does not react is reported as *unobserved*, never as broken. Only a real
    /// lowered control, whose rectangle the host does know, can produce a
    /// confident failure.
    fn usability_press_target(&self, window: WindowId) -> Option<(f32, f32, bool)> {
        let (size, placements) = self.window_placements(window).ok()??;
        for placement in &placements {
            if placement.clickable && placement.width > 1.0 && placement.height > 1.0 {
                return Some((
                    placement.x + placement.width / 2.0,
                    placement.y + placement.height / 2.0,
                    true,
                ));
            }
        }
        Some((size.width as f32 / 2.0, size.height as f32 / 2.0, false))
    }

    fn headless_close_request(&self) -> Option<ui::types::Event> {
        if !self.headless {
            return None;
        }
        // A driven run has its own script and its own ending. Closing it here,
        // after eight quiet waits, would end the watch about a second in and
        // report every well-behaved app as one that closed by itself.
        if self.usability.is_some() {
            return None;
        }
        let waits = self.idle_waits.get().saturating_add(1);
        self.idle_waits.set(waits);
        if waits < HEADLESS_IDLE_WAIT_LIMIT {
            return None;
        }
        let window = self.windows.first().map(|id| id.get()).unwrap_or(0);
        Some(ui::types::Event::CloseRequested(window))
    }

    /// Recompute layout and re-lower supported widgets to native controls.
    ///
    /// This is the naive vertical-slice strategy: every tree change replaces
    /// the whole native widget set. Reconciler diffing comes later.
    fn sync_native_widgets(&self, window: WindowId) -> Result<(), UiDispatchError> {
        let Some((_size, placements)) = self.window_placements(window)? else {
            return Ok(());
        };
        self.dispatcher()
            .lower_widget_placements(window, &placements)?;
        Ok(())
    }

    /// Build the drawn-widget placement list for a window, exactly as
    /// `sync_native_widgets` lowers it, plus the window's logical size.
    ///
    /// Factored out so a headless screenshot can paint the same frame the
    /// native hosts lower -- one source of truth for "what is on screen",
    /// which is the only way a screenshot proves anything about a real run.
    fn window_placements(
        &self,
        window: WindowId,
    ) -> Result<Option<(WindowSize, Vec<WidgetPlacement>)>, UiDispatchError> {
        let dispatcher = self.dispatcher();
        let Some(tree) = dispatcher.widget_tree(window)? else {
            return Ok(None);
        };
        let Some(record) = dispatcher.window(window)? else {
            return Ok(None);
        };

        let viewport = LayoutViewport::new(record.size.width as f32, record.size.height as f32)
            .map_err(|err| UiDispatchError::Layout(err.to_string()))?;
        let layout = dispatcher.compute_layout(window, viewport)?;

        let offsets = self.scroll_offsets.borrow();
        let mut placements = Vec::new();
        for (id, node) in tree.nodes() {
            // One shared list decides what the drawn painters support, so
            // placement filtering and painting can never drift apart.
            if !drawn_kind(node.kind) {
                continue;
            }
            // A widget inside an unselected tab panel is not on screen. The
            // layout collapses the panel, but a nested control still resolves
            // to a rectangle, so without this every tab's contents would paint
            // on top of each other.
            if krate_layout::is_hidden_by_tabs(&tree, *id) {
                continue;
            }
            let Some(rect) = absolute_rect(&tree, &layout, *id) else {
                continue;
            };
            // Widgets inside a Scroll container shift by the container's
            // host-side offset and clip to the container's rectangle.
            let mut y = rect.y;
            let mut clip = None;
            if let Some(scroll_id) = nearest_scroll_ancestor(&tree, *id) {
                if let Some(scroll_rect) = absolute_rect(&tree, &layout, scroll_id) {
                    let offset = offsets.get(&(window, scroll_id)).copied().unwrap_or(0.0);
                    y -= offset;
                    clip = Some((
                        scroll_rect.x,
                        scroll_rect.y,
                        scroll_rect.width,
                        scroll_rect.height,
                    ));
                }
            }
            // Resolve a selectable container's selected index to the child's
            // rect here, where the tree and layout are both in hand; the
            // painters only ever see rectangles. Out-of-range indices and
            // children that failed layout simply draw no highlight.
            let selection = node.selected.and_then(|index| {
                let child = *tree.children(*id).get(index as usize)?;
                let child_rect = absolute_rect(&tree, &layout, child)?;
                Some((
                    child_rect.x,
                    child_rect.y - (rect.y - y),
                    child_rect.width,
                    child_rect.height,
                ))
            });
            // A Text row directly inside a ListView is a selectable row, not a
            // passive label, so mark it clickable. Native hosts lower clickable
            // rows as buttons so a click routes back with the row's widget id;
            // drawn hosts already hit-test every placement and ignore this.
            let list_parent = node
                .parent
                .filter(|_| node.kind == WidgetKind::Text)
                .and_then(|parent| tree.node(parent).map(|p| (parent, p)))
                .filter(|(_, parent)| parent.kind == WidgetKind::ListView);
            let clickable = list_parent.is_some();
            // For a native host, mark the selected row via `checked` so its
            // button can be tinted; the drawn painters use the container's
            // selection wash instead and leave `checked` alone here.
            let row_selected = list_parent.and_then(|(parent_id, parent)| {
                let index = parent.selected?;
                let selected_child = *tree.children(parent_id).get(index as usize)?;
                Some(selected_child == *id)
            });
            placements.push(WidgetPlacement {
                widget: *id,
                kind: node.kind,
                label: node.label.clone(),
                checked: row_selected.or(node.checked),
                value: node.value,
                selection,
                text_cursor: node.text_cursor,
                clip,
                x: rect.x,
                y,
                width: rect.width,
                height: rect.height,
                clickable,
                role: node.role.clone(),
                // Shared, not copied: this runs once per widget per frame, and
                // a photograph is a quarter-gigabyte of pixels.
                pixels: self.images.borrow().get(&(window, *id)).cloned(),
            });
        }
        drop(offsets);

        Ok(Some((record.size, placements)))
    }

    /// Paint a window's current frame to a PNG file, the way a headless
    /// verification or `krate shoot` captures what an app draws.
    ///
    /// This paints the exact placements the native hosts lower, through the
    /// same shared painter, so the image is what a person would see -- not a
    /// separate render path that could drift from the real one.
    pub fn render_window_png(
        &self,
        window: WindowId,
        scale: f32,
        path: &std::path::Path,
    ) -> Result<(), UiDispatchError> {
        let Some((size, placements)) = self.window_placements(window)? else {
            return Err(UiDispatchError::Layout(format!(
                "window {} has nothing to render yet",
                window.get()
            )));
        };
        let scale = scale.max(1.0);
        let pw = ((size.width as f32) * scale).round().max(1.0) as u32;
        let ph = ((size.height as f32) * scale).round().max(1.0) as u32;
        let mut buffer = vec![0u32; pw as usize * ph as usize];
        krate_adapter_common::painter::paint_placements(
            &mut buffer,
            pw,
            ph,
            scale,
            &placements,
            krate_adapter_common::painter::PaintInteraction::default(),
        );
        write_argb_png(&buffer, pw, ph, path)
            .map_err(|err| UiDispatchError::Layout(format!("write {}: {err}", path.display())))
    }

    /// Report a natively lowered control's text whenever a person changes it.
    ///
    /// On hosts that lower to real OS controls, the control holds the text and
    /// the component never sees it. Reading each editable control back after a
    /// pump closes that loop.
    ///
    /// This sends the control's **complete** text, not the part that was added.
    /// An append cannot describe deleting, selecting, or pasting, and trying to
    /// derive one leaves two copies of the text drifting apart. The control is
    /// the single owner; the component mirrors it.
    fn sync_native_text(&self, window: WindowId, dispatcher: &Phase3UiDispatcher<'_>) {
        for widget in dispatcher.native_editable_widgets(window) {
            let Some(current) = dispatcher.native_widget_text(window, widget) else {
                continue;
            };

            let changed = {
                let mut seen = self.native_text.borrow_mut();
                if seen.get(&(window, widget)).map(String::as_str) == Some(current.as_str()) {
                    false
                } else {
                    seen.insert((window, widget), current.clone());
                    true
                }
            };

            if changed {
                let _ = dispatcher.queue_text_changed(window, widget, current);
            }
        }
    }

    /// Forget held keys when a window loses focus.
    ///
    /// Every release from that moment goes to whatever window took focus, so
    /// anything still marked held would stay held forever.
    fn on_window_focus_changed(&self, focused: bool) {
        if !focused {
            self.held_keys.borrow_mut().clear();
        }
    }

    /// Note a close request going past, and say whether the guest has ignored
    /// enough of them that the runtime should act.
    /// Close the window ourselves when the guest ignores the request.
    ///
    /// `windowShouldClose` defers to the app by design, so an app that never
    /// handles `CloseRequested` leaves a window nothing can shut. Every app an
    /// AI has written for us so far is in that category. The first request is
    /// still delivered to the guest untouched -- an app that wants to save on
    /// the way out gets its chance -- and only a request that goes unanswered
    /// while the app keeps running is honoured on its behalf.
    fn close_ignored_by_guest(&self) -> bool {
        let asked = self.close_requests.get();
        if asked < 2 {
            return false;
        }
        // Two presses with the app still looping means it is not listening.
        //
        // Two rather than one on purpose: an app is allowed to answer the
        // first close by saving, asking, or tidying up. But a person who
        // clicks the close button and sees nothing happen clicks it again, so
        // the second press is where patience actually runs out.
        true
    }

    fn note_close_request(&self, event: &ui::types::Event) {
        if matches!(event, ui::types::Event::CloseRequested(_)) {
            self.close_requests.set(self.close_requests.get() + 1);
        }
    }

    fn poll_one_event(&self) -> Result<Option<ui::types::Event>, UiDispatchError> {
        // Anything `key-held` pumped comes out first, in arrival order. It is
        // a real event that simply has not been delivered yet.
        if let Some(event) = self.pending_events.borrow_mut().pop_front() {
            return Ok(Some(event));
        }
        let dispatcher = self.dispatcher();
        for window in &self.windows {
            // Native pumps refresh window state and drain delegate callbacks;
            // headless adapters return no tick. Ignore per-window pump errors
            // so one closed window cannot wedge event delivery.
            let _ = dispatcher.pump_event_loop_once(*window);
            self.sync_native_text(*window, &dispatcher);
        }

        // Route raw native pointer input through layout hit testing so the
        // app-facing event carries a widget id. Raw samples never reach the
        // queue directly, so this cannot loop.
        for sample in dispatcher.drain_raw_pointer_input() {
            if let Some(record) = dispatcher.window(sample.window)? {
                if let Ok(viewport) =
                    LayoutViewport::new(record.size.width as f32, record.size.height as f32)
                {
                    let routed =
                        dispatcher.route_pointer_event(crate::phase3_ui::PointerRouteRequest {
                            window: sample.window,
                            viewport,
                            x: sample.x,
                            y: sample.y,
                            button: Some(PointerButton::Primary),
                            pressed: sample.pressed,
                            modifiers: Modifiers::default(),
                        });
                    // Click-to-focus: a press routed onto a text-entry
                    // widget moves keyboard focus there (queues the
                    // portable focus-changed event through the dispatcher).
                    if sample.pressed {
                        if let Ok(Some(widget)) = routed {
                            if let Ok(Some(tree)) = dispatcher.widget_tree(sample.window) {
                                let focusable = tree
                                    .node(widget)
                                    .is_some_and(|node| press_focuses(node.kind));
                                if focusable
                                    && dispatcher.focused_widget(sample.window).ok().flatten()
                                        != Some(widget)
                                {
                                    let _ = dispatcher.focus_node(sample.window, widget);
                                }
                            }
                        }
                    }
                }
            }
        }

        // Attach keyboard focus to raw key samples and queue portable
        // key/text events. Raw samples never enter the queue directly.
        for sample in dispatcher.drain_raw_key_input() {
            // Held-key state updates from the raw sample rather than the
            // queued event, so a key is held the moment it goes down even if
            // the app never drains the queue -- a game polling `key-held` in a
            // tight frame loop may legitimately never call `poll`.
            {
                let mut held = self.held_keys.borrow_mut();
                if sample.pressed {
                    held.insert(sample.key.clone());
                } else {
                    held.remove(&sample.key);
                }
            }
            let focused = dispatcher.focused_widget(sample.window).ok().flatten();
            if let Ok(event) = krate_adapter_common::ui::KeyEvent::new(
                sample.window,
                focused,
                sample.key.clone(),
                sample.pressed,
                sample.modifiers,
            ) {
                let _ = dispatcher.queue_key_event(event);
            }
            if sample.pressed {
                if let Some(text) = sample.text.as_deref() {
                    if let Ok(event) =
                        krate_adapter_common::ui::TextInputEvent::new(sample.window, focused, text)
                    {
                        let _ = dispatcher.queue_text_input(event);
                    }
                }
            }
        }

        // Wheel input goes two places. A `scroll` container scrolls host-side:
        // hit-test the topmost one under the cursor, clamp its offset to the
        // content extent, and re-lower, so a widget-tree app scrolls the way
        // the platform does without writing any code.
        //
        // The guest also gets the event, always. An app that draws its own
        // content has no scroll container to find, so if this were the only
        // handling every canvas app would silently swallow every scroll -- and
        // that is exactly the bug: a list of 32 items showing 6, with the rest
        // permanently out of reach.
        for sample in dispatcher.drain_raw_wheel_input() {
            let Ok(Some(record)) = dispatcher.window(sample.window) else {
                continue;
            };
            let Ok(viewport) =
                LayoutViewport::new(record.size.width as f32, record.size.height as f32)
            else {
                continue;
            };
            let _ = dispatcher.route_wheel_event(crate::phase3_ui::WheelRouteRequest {
                window: sample.window,
                viewport,
                x: sample.x,
                y: sample.y,
                dx: sample.dx,
                dy: sample.dy,
                modifiers: sample.modifiers,
            });
            let Ok(Some(tree)) = dispatcher.widget_tree(sample.window) else {
                continue;
            };
            let Ok(layout) = dispatcher.compute_layout(sample.window, viewport) else {
                continue;
            };
            let Some(scroll_id) = scroll_container_at(&tree, &layout, sample.x, sample.y) else {
                continue;
            };
            let Some(scroll_rect) = absolute_rect(&tree, &layout, scroll_id) else {
                continue;
            };
            let content_bottom = tree
                .nodes()
                .iter()
                .filter(|(child, _)| nearest_scroll_ancestor(&tree, **child) == Some(scroll_id))
                .filter_map(|(child, _)| absolute_rect(&tree, &layout, *child))
                .map(|r| r.y + r.height)
                .fold(scroll_rect.y, f32::max);
            let content_height = content_bottom - scroll_rect.y;
            let mut offsets = self.scroll_offsets.borrow_mut();
            let entry = offsets.entry((sample.window, scroll_id)).or_insert(0.0);
            let updated =
                clamped_scroll_offset(*entry, sample.dy, content_height, scroll_rect.height);
            if (updated - *entry).abs() > f32::EPSILON {
                *entry = updated;
                drop(offsets);
                let _ = self.sync_native_widgets(sample.window);
            }
        }

        // Skip host-side bookkeeping events that have no portable WIT shape.
        while let Some(event) = dispatcher.poll_event()? {
            // A window losing focus means every release from now on goes
            // somewhere else. Forget what was held, or an app polling
            // `key-held` sees a key that will never come back up -- the player
            // who alt-tabbed mid-stride returns to a character still running.
            if let UiEvent::WindowFocused { focused, .. } = event {
                self.on_window_focus_changed(focused);
            }
            if let Some(event) = event_to_wit(event) {
                return Ok(Some(event));
            }
        }
        Ok(None)
    }
}

/// Map a dispatch error into the portable `ui-error` shape.
fn dispatch_error_to_ui_error(err: UiDispatchError) -> ui::types::UiError {
    match err {
        UiDispatchError::PermissionDenied => ui::types::UiError::PermissionDenied,
        UiDispatchError::Adapter(UiAdapterError::Unsupported(message)) => {
            ui::types::UiError::Unsupported(message)
        }
        UiDispatchError::Adapter(UiAdapterError::InvalidWindow { .. }) => {
            ui::types::UiError::InvalidWindow
        }
        UiDispatchError::Adapter(UiAdapterError::InvalidWidgetId { .. }) => {
            ui::types::UiError::InvalidWidget
        }
        other => ui::types::UiError::Platform(other.to_string()),
    }
}

impl Phase3GuiHost {
    /// Resolve a guest-supplied raw window id against the windows this
    /// component created. Guests cannot reference windows they do not own.
    /// The laid-out size of a canvas widget, in logical pixels.
    fn canvas_widget_rect(
        &self,
        window: WindowId,
        widget: WidgetId,
    ) -> Result<(f32, f32), gfx::types::GfxError> {
        let dispatcher = self.dispatcher();
        let tree = match dispatcher.widget_tree(window) {
            Ok(Some(tree)) => tree,
            _ => return Err(gfx::types::GfxError::InvalidTarget),
        };
        match tree.nodes().iter().find(|(id, _)| **id == widget) {
            Some((_, node)) if node.kind == WidgetKind::Canvas => {}
            Some(_) => {
                return Err(gfx::types::GfxError::Unsupported(
                    "canvas2d binds to a widget of kind canvas".to_string(),
                ))
            }
            None => return Err(gfx::types::GfxError::InvalidTarget),
        }
        let record = match dispatcher.window(window) {
            Ok(Some(record)) => record,
            _ => return Err(gfx::types::GfxError::InvalidTarget),
        };
        let viewport =
            match LayoutViewport::new(record.size.width as f32, record.size.height as f32) {
                Ok(viewport) => viewport,
                Err(error) => return Err(gfx::types::GfxError::Platform(error.to_string())),
            };
        let layout = match dispatcher.compute_layout(window, viewport) {
            Ok(layout) => layout,
            Err(error) => return Err(gfx::types::GfxError::Platform(error.to_string())),
        };
        match absolute_rect(&tree, &layout, widget) {
            Some(rect) => Ok((rect.width, rect.height)),
            None => Err(gfx::types::GfxError::InvalidTarget),
        }
    }

    /// Re-fit a bound canvas's pixel buffer to its widget's current laid-out
    /// rect. A no-op when the size has not changed. Called before reporting
    /// `canvas_size` so an app that lays out from the reported size is laying
    /// out from the size it will actually be shown at.
    fn refit_canvas(&self, canvas: u64) -> Result<(), gfx::types::GfxError> {
        let (window, widget) = {
            let canvases = self.canvases.borrow();
            let Some((window, widget, _)) = canvases.get(&canvas) else {
                return Err(gfx::types::GfxError::InvalidTarget);
            };
            (*window, *widget)
        };
        let rect = self.canvas_widget_rect(window, widget)?;
        let mut canvases = self.canvases.borrow_mut();
        let Some((_, _, surface)) = canvases.get_mut(&canvas) else {
            return Err(gfx::types::GfxError::InvalidTarget);
        };
        surface
            .resize(rect.0.max(1.0) as u32, rect.1.max(1.0) as u32)
            .map_err(|error| gfx::types::GfxError::Unsupported(error.to_string()))?;
        Ok(())
    }

    /// Push a canvas's pixels through the image path and re-lower.
    fn publish_canvas(&self, canvas: u64) -> Result<(), gfx::types::GfxError> {
        let (window, widget, image) = {
            let canvases = self.canvases.borrow();
            let Some((window, widget, surface)) = canvases.get(&canvas) else {
                return Err(gfx::types::GfxError::InvalidTarget);
            };
            let image = surface
                .to_image()
                .map_err(|error| gfx::types::GfxError::Platform(error.to_string()))?;
            (*window, *widget, image)
        };
        self.images
            .borrow_mut()
            .insert((window, widget), Arc::new(image));
        self.sync_native_widgets(window)
            .map_err(|error| gfx::types::GfxError::Platform(error.to_string()))
    }

    fn window_id(&self, raw: u64) -> Result<WindowId, ui::types::UiError> {
        self.windows
            .iter()
            .copied()
            .find(|window| window.get() == raw)
            .ok_or(ui::types::UiError::InvalidWindow)
    }
}

/// Nearest Scroll ancestor of a widget, if any.
fn nearest_scroll_ancestor(
    tree: &krate_adapter_common::ui::WidgetTree,
    id: WidgetId,
) -> Option<WidgetId> {
    let mut current = tree.node(id)?.parent;
    while let Some(parent_id) = current {
        let parent = tree.node(parent_id)?;
        if parent.kind == WidgetKind::Scroll {
            return Some(parent_id);
        }
        current = parent.parent;
    }
    None
}

/// Topmost Scroll container whose rectangle contains the logical point.
fn scroll_container_at(
    tree: &krate_adapter_common::ui::WidgetTree,
    layout: &krate_layout::LayoutSnapshot,
    x: f32,
    y: f32,
) -> Option<WidgetId> {
    tree.nodes()
        .iter()
        .rev()
        .filter(|(_, node)| node.kind == WidgetKind::Scroll)
        .find(|(id, _)| {
            absolute_rect(tree, layout, **id)
                .is_some_and(|r| x >= r.x && y >= r.y && x < r.x + r.width && y < r.y + r.height)
        })
        .map(|(id, _)| *id)
}

/// Clamp a scroll offset after applying a wheel delta: never negative,
/// never past the point where the last content row is visible.
fn clamped_scroll_offset(current: f32, dy: f32, content_height: f32, viewport_height: f32) -> f32 {
    let max_offset = (content_height - viewport_height).max(0.0);
    (current + dy).clamp(0.0, max_offset)
}

/// Widget kinds that take keyboard focus from a pointer press.
fn press_focuses(kind: WidgetKind) -> bool {
    matches!(kind, WidgetKind::TextField | WidgetKind::TextArea)
}

fn widget_id(raw: u64) -> Result<WidgetId, ui::types::UiError> {
    WidgetId::new(raw).map_err(|_| ui::types::UiError::InvalidWidget)
}

fn widget_kind_from_wit(kind: ui::types::WidgetKind) -> WidgetKind {
    match kind {
        ui::types::WidgetKind::Stack => WidgetKind::Stack,
        ui::types::WidgetKind::Grid => WidgetKind::Grid,
        ui::types::WidgetKind::Scroll => WidgetKind::Scroll,
        ui::types::WidgetKind::Tabs => WidgetKind::Tabs,
        ui::types::WidgetKind::Button => WidgetKind::Button,
        ui::types::WidgetKind::Checkbox => WidgetKind::Checkbox,
        ui::types::WidgetKind::Radio => WidgetKind::Radio,
        ui::types::WidgetKind::Switch => WidgetKind::Switch,
        ui::types::WidgetKind::Slider => WidgetKind::Slider,
        ui::types::WidgetKind::Progress => WidgetKind::Progress,
        ui::types::WidgetKind::Text => WidgetKind::Text,
        ui::types::WidgetKind::TextField => WidgetKind::TextField,
        ui::types::WidgetKind::TextArea => WidgetKind::TextArea,
        ui::types::WidgetKind::ListView => WidgetKind::ListView,
        ui::types::WidgetKind::TreeView => WidgetKind::TreeView,
        ui::types::WidgetKind::Image => WidgetKind::Image,
        ui::types::WidgetKind::Canvas => WidgetKind::Canvas,
    }
}

fn widget_node_from_wit(node: ui::types::WidgetNode) -> Result<WidgetNode, ui::types::UiError> {
    let id = widget_id(node.id)?;
    let parent = node.parent.map(widget_id).transpose()?;
    let style = WidgetStyle {
        width: node.style.width,
        height: node.style.height,
        grow: node.style.grow,
        padding: node.style.padding,
    };
    if let Some(value) = node.value {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(ui::types::UiError::Unsupported(
                "widget value must be a finite number in 0..=1".to_string(),
            ));
        }
    }
    let kind = widget_kind_from_wit(node.kind);
    if node.selected.is_some() && !kind_is_selectable(kind) {
        return Err(ui::types::UiError::Unsupported(format!(
            "widget kind {kind:?} cannot carry a selected index"
        )));
    }
    // A caret only means something on an editable text widget. Rejecting it
    // elsewhere keeps a stray value from silently riding on, say, a button.
    if node.text_cursor.is_some() && !matches!(kind, WidgetKind::TextArea | WidgetKind::TextField) {
        return Err(ui::types::UiError::Unsupported(format!(
            "widget kind {kind:?} cannot carry a text caret"
        )));
    }

    Ok(WidgetNode {
        id,
        parent,
        kind,
        label: node.label,
        role: node.role,
        style,
        checked: node.checked,
        value: node.value,
        selected: node.selected,
        text_cursor: node.text_cursor.map(|tc| (tc.cursor, tc.anchor)),
        // A picture arrives through `krate:ui/image`, keyed by widget id, not
        // as a field here. The node the app sends must stay the exact record
        // it was compiled against.
        pixels: None,
    })
}

fn modifiers_to_wit(modifiers: Modifiers) -> ui::types::Modifiers {
    ui::types::Modifiers {
        shift: modifiers.shift,
        control: modifiers.control,
        alt: modifiers.alt,
        meta: modifiers.meta,
    }
}

fn pointer_button_to_wit(button: PointerButton) -> ui::types::PointerButton {
    match button {
        PointerButton::Primary => ui::types::PointerButton::Primary,
        PointerButton::Secondary => ui::types::PointerButton::Secondary,
        PointerButton::Middle => ui::types::PointerButton::Middle,
        PointerButton::Other => ui::types::PointerButton::Other,
    }
}

fn theme_to_wit(theme: Theme) -> ui::types::Theme {
    match theme {
        Theme::Light => ui::types::Theme::Light,
        Theme::Dark => ui::types::Theme::Dark,
        Theme::Unknown => ui::types::Theme::Unknown,
    }
}

/// Map one shared adapter event into the portable WIT event shape.
///
/// Events without a WIT variant yet (window created/shown, widget bookkeeping)
/// are host-side bookkeeping and are not delivered to apps.
fn event_to_wit(event: UiEvent) -> Option<ui::types::Event> {
    match event {
        UiEvent::WindowCloseRequested(id) => Some(ui::types::Event::CloseRequested(id.get())),
        UiEvent::Resized { size, .. } => Some(ui::types::Event::Resized(ui::types::WindowSize {
            width: size.width,
            height: size.height,
        })),
        UiEvent::RedrawRequested(id) => Some(ui::types::Event::RedrawRequested(id.get())),
        UiEvent::Pointer(pointer) => Some(ui::types::Event::Pointer(ui::types::PointerEvent {
            window: pointer.window.get(),
            widget: pointer.widget.map(|widget| widget.get()),
            x: pointer.x,
            y: pointer.y,
            button: pointer.button.map(pointer_button_to_wit),
            pressed: pointer.pressed,
            modifiers: modifiers_to_wit(pointer.modifiers),
        })),
        UiEvent::Wheel(wheel) => Some(ui::types::Event::Wheel(ui::types::WheelEvent {
            window: wheel.window.get(),
            widget: wheel.widget.map(|widget| widget.get()),
            x: wheel.x,
            y: wheel.y,
            dx: wheel.dx,
            dy: wheel.dy,
            modifiers: modifiers_to_wit(wheel.modifiers),
        })),
        UiEvent::Key(key) => Some(ui::types::Event::Key(ui::types::KeyEvent {
            window: key.window.get(),
            widget: key.widget.map(|widget| widget.get()),
            key: key.key,
            pressed: key.pressed,
            modifiers: modifiers_to_wit(key.modifiers),
        })),
        UiEvent::TextInput(text) => Some(ui::types::Event::TextInput(text.text)),
        UiEvent::TextChanged(changed) => {
            Some(ui::types::Event::TextChanged(ui::types::TextChangedEvent {
                window: changed.window.get(),
                widget: changed.widget.get(),
                text: changed.text,
            }))
        }
        UiEvent::FocusChanged { widget, .. } => {
            Some(ui::types::Event::FocusChanged(Some(widget.get())))
        }
        UiEvent::ThemeChanged { theme } => {
            Some(ui::types::Event::ThemeChanged(theme_to_wit(theme)))
        }
        _ => None,
    }
}

impl ui::types::Host for Phase3GuiHost {}

impl ui::window::Host for Phase3GuiHost {
    fn create(
        &mut self,
        title: String,
        size: ui::types::WindowSize,
    ) -> wasmtime::Result<Result<u64, ui::types::UiError>> {
        let size = match WindowSize::new(size.width, size.height) {
            Ok(size) => size,
            Err(err) => return Ok(Err(ui::types::UiError::Platform(err.to_string()))),
        };
        let options = match WindowOptions::new(title, size) {
            Ok(options) => options,
            Err(err) => return Ok(Err(ui::types::UiError::Platform(err.to_string()))),
        };

        let title_note = options.title.clone();
        match self.dispatcher().create_window(options) {
            Ok(id) => {
                self.windows.push(id);
                // One line to stderr, native backends only. A person running a
                // GUI app from a terminal watched a silent prompt for minutes
                // because the window was invisible; had it said this, the gap
                // between "opened" and "nothing on my screen" would have been
                // one glance wide. Stderr so JSON on stdout stays parseable,
                // and never on the headless path so replay logs stay quiet.
                // Not when the front door is running the app: it has already
                // said which app is opening and how to come back, so this is a
                // second, blunter copy of the same sentence -- and it names the
                // window's own title, which the person can see.
                if !self.headless && std::env::var_os("KRATE_QUIET_LAUNCH").is_none() {
                    eprintln!(
                        "krate: opened window {title_note:?} (close it or press Ctrl-C to quit)"
                    );
                }
                Ok(Ok(id.get()))
            }
            Err(err) => Ok(Err(dispatch_error_to_ui_error(err))),
        }
    }

    fn show(&mut self, window: u64) -> wasmtime::Result<Result<(), ui::types::UiError>> {
        let id = match self.window_id(window) {
            Ok(id) => id,
            Err(err) => return Ok(Err(err)),
        };
        Ok(self
            .dispatcher()
            .show_window(id)
            .map_err(dispatch_error_to_ui_error))
    }

    fn close(&mut self, window: u64) -> wasmtime::Result<Result<(), ui::types::UiError>> {
        let id = match self.window_id(window) {
            Ok(id) => id,
            Err(err) => return Ok(Err(err)),
        };
        // Last chance to capture: an app that builds its window and exits
        // without ever waiting for input -- a scripted verification run, a
        // one-shot tool -- would otherwise never hit the wait or present
        // capture points. Shoot its final frame here, while the tree is still
        // alive, before the window is torn down.
        self.maybe_take_screenshot_for(id);
        let result = self
            .dispatcher()
            .close_window(id)
            .map_err(dispatch_error_to_ui_error);
        self.windows.retain(|tracked| *tracked != id);
        Ok(result)
    }

    fn set_title(
        &mut self,
        window: u64,
        title: String,
    ) -> wasmtime::Result<Result<(), ui::types::UiError>> {
        let id = match self.window_id(window) {
            Ok(id) => id,
            Err(err) => return Ok(Err(err)),
        };
        Ok(self
            .dispatcher()
            .set_title(id, title)
            .map_err(dispatch_error_to_ui_error))
    }

    fn set_size(
        &mut self,
        window: u64,
        size: ui::types::WindowSize,
    ) -> wasmtime::Result<Result<(), ui::types::UiError>> {
        let id = match self.window_id(window) {
            Ok(id) => id,
            Err(err) => return Ok(Err(err)),
        };
        let size = match WindowSize::new(size.width, size.height) {
            Ok(size) => size,
            Err(err) => return Ok(Err(ui::types::UiError::Platform(err.to_string()))),
        };
        Ok(self
            .dispatcher()
            .set_size(id, size)
            .map_err(dispatch_error_to_ui_error))
    }

    fn set_state(
        &mut self,
        _window: u64,
        _state: ui::types::WindowState,
    ) -> wasmtime::Result<Result<(), ui::types::UiError>> {
        Ok(Err(ui::types::UiError::Unsupported(
            "window state changes are not implemented yet".to_string(),
        )))
    }

    fn request_redraw(&mut self, window: u64) -> wasmtime::Result<Result<(), ui::types::UiError>> {
        let id = match self.window_id(window) {
            Ok(id) => id,
            Err(err) => return Ok(Err(err)),
        };
        Ok(self
            .dispatcher()
            .request_redraw(id)
            .map_err(dispatch_error_to_ui_error))
    }
}

impl ui::tree::Host for Phase3GuiHost {
    fn set_root(
        &mut self,
        window: u64,
        root: ui::types::WidgetNode,
    ) -> wasmtime::Result<Result<(), ui::types::UiError>> {
        let id = match self.window_id(window) {
            Ok(id) => id,
            Err(err) => return Ok(Err(err)),
        };
        let root = match widget_node_from_wit(root) {
            Ok(root) => root,
            Err(err) => return Ok(Err(err)),
        };
        if let Err(err) = self.dispatcher().set_root(id, root) {
            return Ok(Err(dispatch_error_to_ui_error(err)));
        }
        Ok(self
            .sync_native_widgets(id)
            .map_err(dispatch_error_to_ui_error))
    }

    fn upsert_node(
        &mut self,
        window: u64,
        node: ui::types::WidgetNode,
    ) -> wasmtime::Result<Result<(), ui::types::UiError>> {
        let id = match self.window_id(window) {
            Ok(id) => id,
            Err(err) => return Ok(Err(err)),
        };
        let node = match widget_node_from_wit(node) {
            Ok(node) => node,
            Err(err) => return Ok(Err(err)),
        };
        if let Err(err) = self.dispatcher().upsert_node(id, node) {
            return Ok(Err(dispatch_error_to_ui_error(err)));
        }
        Ok(self
            .sync_native_widgets(id)
            .map_err(dispatch_error_to_ui_error))
    }

    fn remove_node(
        &mut self,
        window: u64,
        widget: u64,
    ) -> wasmtime::Result<Result<(), ui::types::UiError>> {
        let id = match self.window_id(window) {
            Ok(id) => id,
            Err(err) => return Ok(Err(err)),
        };
        let Ok(widget) = widget_id(widget) else {
            return Ok(Err(ui::types::UiError::InvalidWidget));
        };
        if let Err(err) = self.dispatcher().remove_node(id, widget) {
            return Ok(Err(dispatch_error_to_ui_error(err)));
        }
        Ok(self
            .sync_native_widgets(id)
            .map_err(dispatch_error_to_ui_error))
    }

    fn focus_node(
        &mut self,
        window: u64,
        widget: u64,
    ) -> wasmtime::Result<Result<(), ui::types::UiError>> {
        let id = match self.window_id(window) {
            Ok(id) => id,
            Err(err) => return Ok(Err(err)),
        };
        let Ok(widget) = widget_id(widget) else {
            return Ok(Err(ui::types::UiError::InvalidWidget));
        };
        Ok(self
            .dispatcher()
            .focus_node(id, widget)
            .map_err(dispatch_error_to_ui_error))
    }

    fn set_enabled(
        &mut self,
        _window: u64,
        _widget: u64,
        _enabled: bool,
    ) -> wasmtime::Result<Result<(), ui::types::UiError>> {
        Ok(Err(ui::types::UiError::Unsupported(
            "widget enable state is not implemented yet".to_string(),
        )))
    }
}

impl ui::events::Host for Phase3GuiHost {
    fn poll(&mut self) -> wasmtime::Result<Option<ui::types::Event>> {
        // No screenshot here for the same reason as key_held: poll is called at
        // the top of a frame loop, before this frame is drawn. wait (widget
        // apps) and present (canvas and scene apps) are the capture points.
        self.pump_native_windows();
        // A game never calls `wait` -- it polls at the top of every frame and
        // draws regardless. Driving only from `wait` means such an app is never
        // stepped and never released, so the run hangs until the outer
        // watchdog kills it and the stage blames the app for a defect it does
        // not have. Stepping here too is what makes the driver work for a
        // frame loop as well as an event loop.
        if self.usability.is_some() {
            if let Some(event) = self.drive_usability_step() {
                return Ok(Some(event));
            }
        }
        if interrupted() {
            // First interrupt is reported as a close, so an app that saves on
            // the way out does. An app that ignores it keeps looping and comes
            // straight back here, so the second press ends the process --
            // otherwise Ctrl-C is a suggestion the app can decline.
            self.interrupts.set(self.interrupts.get() + 1);
            if self.interrupts.get() > 1 {
                std::process::exit(130);
            }
            if let Some(window) = self.windows.first().copied() {
                return Ok(Some(ui::types::Event::CloseRequested(window.get())));
            }
        }
        let event = self
            .poll_one_event()
            .map_err(|err| wasmtime::Error::msg(err.to_string()))?;
        if let Some(event) = &event {
            self.note_close_request(event);
        }
        if self.close_ignored_by_guest() {
            return Ok(None);
        }
        Ok(event)
    }

    fn key_held(&mut self, key: String) -> wasmtime::Result<bool> {
        // No screenshot here: a game reads key-held at the top of its loop,
        // before it has drawn this frame's scene, so capturing on the first
        // key-held grabs a blank frame. The scene `present` captures instead.
        //
        // Pump first: a game that only ever calls `key-held` in a tight frame
        // loop never drains the queue, and without this its input would be
        // whatever arrived before the last `poll`.
        //
        // Keep whatever the pump produced. Discarding it here is what stopped
        // the close button working: a game reading ten keys a frame pumped ten
        // times, and the CloseRequested that came out of one of them was
        // thrown away before the game's own `poll` could match on it.
        if let Ok(Some(event)) = self.poll_one_event() {
            self.pending_events.borrow_mut().push_back(event);
        }
        Ok(self.held_keys.borrow().contains(&key))
    }

    fn gamepad_connected(&mut self) -> wasmtime::Result<bool> {
        Ok(self.gamepads.borrow_mut().connected())
    }

    fn gamepad_held(&mut self, button: String) -> wasmtime::Result<bool> {
        Ok(self.gamepads.borrow_mut().held(&button))
    }

    fn gamepad_axis(&mut self, axis: String) -> wasmtime::Result<f32> {
        Ok(self.gamepads.borrow_mut().axis(&axis))
    }

    fn wait(&mut self, timeout_millis: Option<u32>) -> wasmtime::Result<Option<ui::types::Event>> {
        // The app has built its window and is now waiting for input: the first
        // stable, fully-drawn frame. Capture here, before any budget check
        // that might end the run.
        self.maybe_take_screenshot();

        let deadline = timeout_millis
            .map(|ms| std::time::Instant::now() + std::time::Duration::from_millis(u64::from(ms)));

        // A driven run is scripted, so it owns the budget question outright:
        // the driver decides when this run has seen enough and ends it itself.
        // The ordinary wall-clock budget below would cut the run off at five
        // seconds, which is before the ten-second self-close the driver exists
        // to catch.
        if self.usability.is_some() {
            if let Some(event) = self.drive_usability_step() {
                self.idle_waits.set(0);
                return Ok(Some(event));
            }
        } else if let Some(close) = self.headless_budget_close_request() {
            // Checked before polling, deliberately. An animation loop calls
            // `request-redraw` every frame and immediately receives that redraw
            // back, so the queue is never empty and the app looks busy forever
            // -- but nothing is happening that a person would recognise as
            // activity. The budget is wall-clock precisely so a loop cannot
            // feed itself past it.
            self.idle_waits.set(0);
            return Ok(Some(close));
        }

        loop {
            if interrupted() {
                self.interrupts.set(self.interrupts.get() + 1);
                if self.interrupts.get() > 1 {
                    std::process::exit(130);
                }
                if let Some(window) = self.windows.first().copied() {
                    return Ok(Some(ui::types::Event::CloseRequested(window.get())));
                }
            }
            self.pump_native_windows();
            let event = self
                .poll_one_event()
                .map_err(|err| wasmtime::Error::msg(err.to_string()))?;
            if event.is_some() {
                self.idle_waits.set(0);
                return Ok(event);
            }
            if let Some(deadline) = deadline {
                if std::time::Instant::now() >= deadline {
                    // The guest asked for a timeout and it has arrived. Hand
                    // control back exactly as asked; it is the guest's own loop
                    // bound that ends the run, and it stays free to treat the
                    // quiet round however it likes.
                    return Ok(None);
                }
            } else if let Some(close) = self.headless_close_request() {
                // An unbounded wait on a headless host can never return on its
                // own: nothing exists that could deliver an event. Reporting
                // the close is the only honest way out, and every GUI app
                // already handles it, so the app shuts down through its normal
                // path and still saves on the way out.
                return Ok(Some(close));
            }
            std::thread::sleep(std::time::Duration::from_millis(WAIT_POLL_INTERVAL_MILLIS));
        }
    }
}

impl ui::launcher::Host for Phase3GuiHost {
    fn open_url(&mut self, url: String) -> wasmtime::Result<Result<(), ui::launcher::LaunchError>> {
        // Checked before the URL is even looked at, so a denied app cannot use
        // the difference between error messages to probe what would be allowed.
        let granted = self
            .runtime
            .guard()
            .check(&UapiCall::Ui(UiCall::OpenUrl))
            .is_ok();
        Ok(
            crate::desktop_host::open_url(&url, granted).map_err(|err| match err {
                crate::desktop_host::LaunchError::Denied => ui::launcher::LaunchError::Denied,
                crate::desktop_host::LaunchError::InvalidUrl(m) => {
                    ui::launcher::LaunchError::InvalidUrl(m)
                }
                crate::desktop_host::LaunchError::Unavailable(m) => {
                    ui::launcher::LaunchError::Unavailable(m)
                }
            }),
        )
    }
}

impl ui::notify::Host for Phase3GuiHost {
    fn show(
        &mut self,
        title: String,
        body: String,
    ) -> wasmtime::Result<Result<(), ui::notify::NotifyError>> {
        let granted = self
            .runtime
            .guard()
            .check(&UapiCall::Ui(UiCall::Notify))
            .is_ok();
        // The app's own title is used for attribution, so a notification cannot
        // be made to look like it came from somewhere else.
        Ok(
            crate::desktop_host::notify(&title, &body, &title, granted).map_err(|err| match err {
                crate::desktop_host::NotifyError::Denied => ui::notify::NotifyError::Denied,
                crate::desktop_host::NotifyError::InvalidContent(m) => {
                    ui::notify::NotifyError::InvalidContent(m)
                }
                crate::desktop_host::NotifyError::Unavailable(m) => {
                    ui::notify::NotifyError::Unavailable(m)
                }
            }),
        )
    }
}

impl ui::dialog::Host for Phase3GuiHost {
    /// Show the system's open-file dialog and remember what was chosen.
    ///
    /// The app gets a name and a token, never a path. That is what makes the
    /// click a grant rather than a hole: it can open the one file the person
    /// picked, and cannot read its siblings, walk to its folder, or store the
    /// location for a later run.
    fn open_file(
        &mut self,
        _window: u64,
        title: String,
        filter: String,
    ) -> wasmtime::Result<Result<Option<ui::dialog::ChosenFile>, ui::types::UiError>> {
        let chosen = match choose_file_on_host(&title, &filter) {
            Ok(Some(path)) => path,
            // Cancelling is a normal answer, not a failure.
            Ok(None) => return Ok(Ok(None)),
            Err(err) => return Ok(Err(ui::types::UiError::Unsupported(err))),
        };

        let name = crate::chosen_files::ChosenFiles::display_name(&chosen);
        let Some(token) = self.chosen_files.borrow_mut().remember(chosen) else {
            return Ok(Err(ui::types::UiError::Unsupported(
                "too many files chosen in one run".to_string(),
            )));
        };
        Ok(Ok(Some(ui::dialog::ChosenFile { name, token })))
    }

    /// Show a message and wait for the person to dismiss it.
    fn message(
        &mut self,
        _window: u64,
        title: String,
        body: String,
    ) -> wasmtime::Result<Result<(), ui::types::UiError>> {
        rfd::MessageDialog::new()
            .set_title(&title)
            .set_description(&body)
            .set_buttons(rfd::MessageButtons::Ok)
            .show();
        Ok(Ok(()))
    }

    /// Ask a yes/no question and return what the person chose.
    ///
    /// A dismissed dialog counts as "no": an app that treats silence as consent
    /// is doing something the person did not agree to.
    fn confirm(
        &mut self,
        _window: u64,
        title: String,
        body: String,
    ) -> wasmtime::Result<Result<bool, ui::types::UiError>> {
        let answer = rfd::MessageDialog::new()
            .set_title(&title)
            .set_description(&body)
            .set_buttons(rfd::MessageButtons::YesNo)
            .show();
        Ok(Ok(answer == rfd::MessageDialogResult::Yes))
    }
}

impl ui::image::Host for Phase3GuiHost {
    fn set_pixels(
        &mut self,
        window: u64,
        widget: u64,
        pixels: ui::image::ImagePixels,
    ) -> wasmtime::Result<Result<(), ui::types::UiError>> {
        let window_id = match self.window_id(window) {
            Ok(id) => id,
            Err(err) => return Ok(Err(err)),
        };
        let widget_id = match WidgetId::new(widget) {
            Ok(id) => id,
            Err(err) => return Ok(Err(ui::types::UiError::Unsupported(err.to_string()))),
        };

        // A picture only means something on an image widget. Accepting one for
        // a button would store a buffer nothing ever draws and leave the app
        // believing it had shown something.
        match self.dispatcher().widget_tree(window_id) {
            Ok(Some(tree)) => match tree.nodes().iter().find(|(id, _)| **id == widget_id) {
                Some((_, node)) if node.kind == WidgetKind::Image => {}
                Some((_, node)) => {
                    return Ok(Err(ui::types::UiError::Unsupported(format!(
                        "widget kind {:?} cannot show a picture",
                        node.kind
                    ))))
                }
                None => {
                    return Ok(Err(ui::types::UiError::Unsupported(format!(
                        "window {window} has no widget {widget}"
                    ))))
                }
            },
            Ok(None) => {
                return Ok(Err(ui::types::UiError::Unsupported(format!(
                    "window {window} has no widgets yet"
                ))))
            }
            Err(err) => return Ok(Err(dispatch_error_to_ui_error(err))),
        }

        let image = match ImagePixels::new(pixels.width, pixels.height, pixels.rgba) {
            Ok(image) => image,
            Err(err) => return Ok(Err(ui::types::UiError::Unsupported(err.to_string()))),
        };
        self.images
            .borrow_mut()
            .insert((window_id, widget_id), Arc::new(image));
        Ok(self
            .sync_native_widgets(window_id)
            .map_err(dispatch_error_to_ui_error))
    }

    fn clear(
        &mut self,
        window: u64,
        widget: u64,
    ) -> wasmtime::Result<Result<(), ui::types::UiError>> {
        let window_id = match self.window_id(window) {
            Ok(id) => id,
            Err(err) => return Ok(Err(err)),
        };
        let widget_id = match WidgetId::new(widget) {
            Ok(id) => id,
            Err(err) => return Ok(Err(ui::types::UiError::Unsupported(err.to_string()))),
        };
        // Clearing a widget that has no picture is not an error: an app
        // resetting its view should not have to remember whether it ever set
        // one.
        self.images.borrow_mut().remove(&(window_id, widget_id));
        Ok(self
            .sync_native_widgets(window_id)
            .map_err(dispatch_error_to_ui_error))
    }
}

impl ui::clipboard::Host for Phase3GuiHost {
    fn read_text(&mut self) -> wasmtime::Result<Result<String, ui::types::UiError>> {
        Ok(self
            .dispatcher()
            .read_clipboard_text()
            .map_err(dispatch_error_to_ui_error))
    }

    fn write_text(&mut self, text: String) -> wasmtime::Result<Result<(), ui::types::UiError>> {
        Ok(self
            .dispatcher()
            .write_clipboard_text(&text)
            .map_err(dispatch_error_to_ui_error))
    }
}

impl ui::menu::Host for Phase3GuiHost {
    fn set_items(
        &mut self,
        _window: u64,
        _items: Vec<ui::types::MenuItem>,
    ) -> wasmtime::Result<Result<(), ui::types::UiError>> {
        Ok(Err(ui::types::UiError::Unsupported(
            "menus are not implemented yet".to_string(),
        )))
    }
}

impl gfx::types::Host for Phase3GuiHost {}

impl gfx::canvas2d::Host for Phase3GuiHost {
    fn bind(
        &mut self,
        window: u64,
        widget: u64,
    ) -> wasmtime::Result<Result<u64, gfx::types::GfxError>> {
        let window_id = match self.window_id(window) {
            Ok(id) => id,
            Err(_) => return Ok(Err(gfx::types::GfxError::InvalidTarget)),
        };
        let Ok(widget_id) = WidgetId::new(widget) else {
            return Ok(Err(gfx::types::GfxError::InvalidTarget));
        };

        // The canvas takes the widget's laid-out size, so what the app draws
        // is what the layout gave it -- the same rect every host will show.
        let rect = match self.canvas_widget_rect(window_id, widget_id) {
            Ok(rect) => rect,
            Err(error) => return Ok(Err(error)),
        };
        let surface = match CanvasSurface::new(rect.0.max(1.0) as u32, rect.1.max(1.0) as u32) {
            Ok(surface) => surface,
            Err(error) => return Ok(Err(gfx::types::GfxError::Unsupported(error.to_string()))),
        };

        let canvas_id = self.next_canvas_id.get();
        self.next_canvas_id.set(canvas_id.saturating_add(1));
        self.canvases
            .borrow_mut()
            .insert(canvas_id, (window_id, widget_id, surface));
        Ok(Ok(canvas_id))
    }

    fn canvas_size(
        &mut self,
        canvas: u64,
    ) -> wasmtime::Result<Result<gfx::types::Size, gfx::types::GfxError>> {
        // Re-fit to the widget's current rect before answering. The window is
        // resizable, so the rect the canvas was bound to is not the rect it
        // still has. Asking is how an app learns it was resized, and a stale
        // answer here is what makes every hit-box drift after a resize.
        if let Err(error) = self.refit_canvas(canvas) {
            return Ok(Err(error));
        }
        let canvases = self.canvases.borrow();
        let Some((_, _, surface)) = canvases.get(&canvas) else {
            return Ok(Err(gfx::types::GfxError::InvalidTarget));
        };
        let (width, height) = surface.dimensions();
        Ok(Ok(gfx::types::Size {
            width: width as f32,
            height: height as f32,
        }))
    }

    fn clear(
        &mut self,
        canvas: u64,
        fill: gfx::types::Color,
    ) -> wasmtime::Result<Result<(), gfx::types::GfxError>> {
        let mut canvases = self.canvases.borrow_mut();
        let Some((_, _, surface)) = canvases.get_mut(&canvas) else {
            return Ok(Err(gfx::types::GfxError::InvalidTarget));
        };
        surface.clear(pack_color(fill.r, fill.g, fill.b, fill.a));
        Ok(Ok(()))
    }

    fn set_clip(
        &mut self,
        canvas: u64,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
    ) -> wasmtime::Result<Result<(), gfx::types::GfxError>> {
        let mut canvases = self.canvases.borrow_mut();
        let Some((_, _, surface)) = canvases.get_mut(&canvas) else {
            return Ok(Err(gfx::types::GfxError::InvalidTarget));
        };
        // A zero or negative rect clips everything away rather than being an
        // error: an app computing a region from a resized window can land
        // there legitimately, and drawing nothing is the right answer.
        surface.set_clip(Some((x, y, w.max(0.0), h.max(0.0))));
        Ok(Ok(()))
    }

    fn clear_clip(&mut self, canvas: u64) -> wasmtime::Result<Result<(), gfx::types::GfxError>> {
        let mut canvases = self.canvases.borrow_mut();
        let Some((_, _, surface)) = canvases.get_mut(&canvas) else {
            return Ok(Err(gfx::types::GfxError::InvalidTarget));
        };
        surface.set_clip(None);
        Ok(Ok(()))
    }

    fn fill_rect(
        &mut self,
        canvas: u64,
        area: gfx::types::Rect,
        fill: gfx::types::Color,
    ) -> wasmtime::Result<Result<(), gfx::types::GfxError>> {
        let mut canvases = self.canvases.borrow_mut();
        let Some((_, _, surface)) = canvases.get_mut(&canvas) else {
            return Ok(Err(gfx::types::GfxError::InvalidTarget));
        };
        surface.fill_rect(
            area.x,
            area.y,
            area.width,
            area.height,
            pack_color(fill.r, fill.g, fill.b, fill.a),
        );
        Ok(Ok(()))
    }

    fn fill_circle(
        &mut self,
        canvas: u64,
        center: gfx::types::Point,
        radius: f32,
        fill: gfx::types::Color,
    ) -> wasmtime::Result<Result<(), gfx::types::GfxError>> {
        let mut canvases = self.canvases.borrow_mut();
        let Some((_, _, surface)) = canvases.get_mut(&canvas) else {
            return Ok(Err(gfx::types::GfxError::InvalidTarget));
        };
        surface.fill_circle(
            center.x,
            center.y,
            radius,
            pack_color(fill.r, fill.g, fill.b, fill.a),
        );
        Ok(Ok(()))
    }

    fn stroke_circle(
        &mut self,
        canvas: u64,
        center: gfx::types::Point,
        radius: f32,
        width: f32,
        stroke: gfx::types::Color,
    ) -> wasmtime::Result<Result<(), gfx::types::GfxError>> {
        let mut canvases = self.canvases.borrow_mut();
        let Some((_, _, surface)) = canvases.get_mut(&canvas) else {
            return Ok(Err(gfx::types::GfxError::InvalidTarget));
        };
        surface.stroke_circle(
            center.x,
            center.y,
            radius,
            width,
            pack_color(stroke.r, stroke.g, stroke.b, stroke.a),
        );
        Ok(Ok(()))
    }

    fn fill_round_rect(
        &mut self,
        canvas: u64,
        area: gfx::types::Rect,
        radii: gfx::types::CornerRadii,
        fill: gfx::types::Color,
    ) -> wasmtime::Result<Result<(), gfx::types::GfxError>> {
        let mut canvases = self.canvases.borrow_mut();
        let Some((_, _, surface)) = canvases.get_mut(&canvas) else {
            return Ok(Err(gfx::types::GfxError::InvalidTarget));
        };
        surface.fill_round_rect(
            area.x,
            area.y,
            area.width,
            area.height,
            (
                radii.top_left,
                radii.top_right,
                radii.bottom_right,
                radii.bottom_left,
            ),
            pack_color(fill.r, fill.g, fill.b, fill.a),
        );
        Ok(Ok(()))
    }

    fn stroke_round_rect(
        &mut self,
        canvas: u64,
        area: gfx::types::Rect,
        radii: gfx::types::CornerRadii,
        width: f32,
        stroke: gfx::types::Color,
    ) -> wasmtime::Result<Result<(), gfx::types::GfxError>> {
        let mut canvases = self.canvases.borrow_mut();
        let Some((_, _, surface)) = canvases.get_mut(&canvas) else {
            return Ok(Err(gfx::types::GfxError::InvalidTarget));
        };
        surface.stroke_round_rect(
            area.x,
            area.y,
            area.width,
            area.height,
            (
                radii.top_left,
                radii.top_right,
                radii.bottom_right,
                radii.bottom_left,
            ),
            width,
            pack_color(stroke.r, stroke.g, stroke.b, stroke.a),
        );
        Ok(Ok(()))
    }

    fn drop_shadow_round_rect(
        &mut self,
        canvas: u64,
        area: gfx::types::Rect,
        radii: gfx::types::CornerRadii,
        blur: f32,
        shadow: gfx::types::Color,
    ) -> wasmtime::Result<Result<(), gfx::types::GfxError>> {
        let mut canvases = self.canvases.borrow_mut();
        let Some((_, _, surface)) = canvases.get_mut(&canvas) else {
            return Ok(Err(gfx::types::GfxError::InvalidTarget));
        };
        surface.drop_shadow_round_rect(
            area.x,
            area.y,
            area.width,
            area.height,
            (
                radii.top_left,
                radii.top_right,
                radii.bottom_right,
                radii.bottom_left,
            ),
            blur,
            pack_color(shadow.r, shadow.g, shadow.b, shadow.a),
        );
        Ok(Ok(()))
    }

    fn linear_gradient_stops(
        &mut self,
        canvas: u64,
        area: gfx::types::Rect,
        angle_degrees: f32,
        stops: Vec<gfx::types::GradientStop>,
    ) -> wasmtime::Result<Result<(), gfx::types::GfxError>> {
        let mut canvases = self.canvases.borrow_mut();
        let Some((_, _, surface)) = canvases.get_mut(&canvas) else {
            return Ok(Err(gfx::types::GfxError::InvalidTarget));
        };
        let packed: Vec<(f32, u32)> = stops
            .iter()
            .map(|s| {
                (
                    s.offset,
                    pack_color(s.color.r, s.color.g, s.color.b, s.color.a),
                )
            })
            .collect();
        surface.linear_gradient_stops(
            area.x,
            area.y,
            area.width,
            area.height,
            angle_degrees,
            &packed,
        );
        Ok(Ok(()))
    }

    fn radial_gradient(
        &mut self,
        canvas: u64,
        center: gfx::types::Point,
        radius: f32,
        inner: gfx::types::Color,
        outer: gfx::types::Color,
    ) -> wasmtime::Result<Result<(), gfx::types::GfxError>> {
        let mut canvases = self.canvases.borrow_mut();
        let Some((_, _, surface)) = canvases.get_mut(&canvas) else {
            return Ok(Err(gfx::types::GfxError::InvalidTarget));
        };
        surface.radial_gradient(
            center.x,
            center.y,
            radius,
            pack_color(inner.r, inner.g, inner.b, inner.a),
            pack_color(outer.r, outer.g, outer.b, outer.a),
        );
        Ok(Ok(()))
    }

    fn linear_gradient(
        &mut self,
        canvas: u64,
        area: gfx::types::Rect,
        top: gfx::types::Color,
        bottom: gfx::types::Color,
    ) -> wasmtime::Result<Result<(), gfx::types::GfxError>> {
        let mut canvases = self.canvases.borrow_mut();
        let Some((_, _, surface)) = canvases.get_mut(&canvas) else {
            return Ok(Err(gfx::types::GfxError::InvalidTarget));
        };
        surface.linear_gradient_v(
            area.x,
            area.y,
            area.width,
            area.height,
            pack_color(top.r, top.g, top.b, top.a),
            pack_color(bottom.r, bottom.g, bottom.b, bottom.a),
        );
        Ok(Ok(()))
    }

    fn stroke_rect(
        &mut self,
        canvas: u64,
        area: gfx::types::Rect,
        stroke: gfx::types::Color,
        width: f32,
    ) -> wasmtime::Result<Result<(), gfx::types::GfxError>> {
        let mut canvases = self.canvases.borrow_mut();
        let Some((_, _, surface)) = canvases.get_mut(&canvas) else {
            return Ok(Err(gfx::types::GfxError::InvalidTarget));
        };
        surface.stroke_rect(
            area.x,
            area.y,
            area.width,
            area.height,
            width,
            pack_color(stroke.r, stroke.g, stroke.b, stroke.a),
        );
        Ok(Ok(()))
    }

    fn draw_text(
        &mut self,
        canvas: u64,
        text: String,
        origin: gfx::types::Point,
        font_size: f32,
        ink: gfx::types::Color,
    ) -> wasmtime::Result<Result<(), gfx::types::GfxError>> {
        let mut canvases = self.canvases.borrow_mut();
        let Some((_, _, surface)) = canvases.get_mut(&canvas) else {
            return Ok(Err(gfx::types::GfxError::InvalidTarget));
        };
        surface.text(
            &text,
            origin.x,
            origin.y,
            font_size,
            pack_color(ink.r, ink.g, ink.b, ink.a),
        );
        Ok(Ok(()))
    }

    fn measure_text(
        &mut self,
        canvas: u64,
        text: String,
        font_size: f32,
    ) -> wasmtime::Result<Result<gfx::types::TextMetrics, gfx::types::GfxError>> {
        let canvases = self.canvases.borrow();
        let Some((_, _, surface)) = canvases.get(&canvas) else {
            return Ok(Err(gfx::types::GfxError::InvalidTarget));
        };
        // Measured through the same surface that will draw it, so the answer
        // comes from whichever face -- vector or bitmap fallback -- this host
        // actually paints with.
        let m = surface.measure_text(&text, font_size);
        Ok(Ok(gfx::types::TextMetrics {
            width: m.width,
            height: m.height,
            ascent: m.ascent,
            descent: m.descent,
        }))
    }

    fn draw_pixels(
        &mut self,
        canvas: u64,
        area: gfx::types::Rect,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    ) -> wasmtime::Result<Result<(), gfx::types::GfxError>> {
        // The guest is the untrusted side: a buffer shorter than its stated
        // size would read past the end on the last row. ImagePixels checks
        // that once, here, rather than in the sampling loop.
        let image = match ImagePixels::new(width, height, rgba) {
            Ok(image) => image,
            Err(error) => return Ok(Err(gfx::types::GfxError::Unsupported(error.to_string()))),
        };
        let mut canvases = self.canvases.borrow_mut();
        let Some((_, _, surface)) = canvases.get_mut(&canvas) else {
            return Ok(Err(gfx::types::GfxError::InvalidTarget));
        };
        surface.draw_pixels(area.x, area.y, area.width, area.height, &image);
        Ok(Ok(()))
    }

    fn draw_sprite(
        &mut self,
        canvas: u64,
        center: gfx::types::Point,
        dst: gfx::types::Size,
        angle: f32,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    ) -> wasmtime::Result<Result<(), gfx::types::GfxError>> {
        let image = match ImagePixels::new(width, height, rgba) {
            Ok(image) => image,
            Err(error) => return Ok(Err(gfx::types::GfxError::Unsupported(error.to_string()))),
        };
        let mut canvases = self.canvases.borrow_mut();
        let Some((_, _, surface)) = canvases.get_mut(&canvas) else {
            return Ok(Err(gfx::types::GfxError::InvalidTarget));
        };
        surface.draw_sprite(center.x, center.y, dst.width, dst.height, angle, &image);
        Ok(Ok(()))
    }

    fn present(&mut self, canvas: u64) -> wasmtime::Result<Result<(), gfx::types::GfxError>> {
        // The one call that reaches the widget. Draw calls mutate the raster;
        // this publishes it, so a hundred fills cost one render.
        // Pace the frame. A game loop calls poll and present with no sleep of
        // its own, which spins a core flat out to produce frames no display
        // can show -- a laptop's fan tells you before the frame rate does.
        // Sleeping the remainder of a 60Hz budget here means every app is
        // paced without every author having to think about it, and an app that
        // is already slower than the budget waits for nothing.
        //
        // Deliberately not in the guest: an app that forgets to sleep should
        // not be able to melt the machine, and one that sleeps by hand still
        // works because this only ever waits for the time left over.
        // A game loop lives in present, not in wait or poll, so an interrupt
        // has to be answered here as well or Ctrl-C does nothing to a game.
        if interrupted() {
            self.interrupts.set(self.interrupts.get() + 1);
            if self.interrupts.get() > 1 {
                std::process::exit(130);
            }
        }
        const FRAME_BUDGET: std::time::Duration = std::time::Duration::from_micros(16_667);
        if let Some(previous) = self.last_present.get() {
            let elapsed = previous.elapsed();
            if elapsed < FRAME_BUDGET {
                std::thread::sleep(FRAME_BUDGET - elapsed);
            }
        }
        self.last_present.set(Some(std::time::Instant::now()));

        let result = self.publish_canvas(canvas);
        // A presented canvas is a real drawn frame -- the right moment to
        // capture a 2D game or drawing, which may never call wait.
        if result.is_ok() {
            self.maybe_take_screenshot();
        }
        Ok(result)
    }
}

impl gfx::scene3d::Host for Phase3GuiHost {
    fn bind(
        &mut self,
        window: u64,
        widget: u64,
    ) -> wasmtime::Result<Result<u64, gfx::types::GfxError>> {
        let window_id = match self.window_id(window) {
            Ok(id) => id,
            Err(_) => return Ok(Err(gfx::types::GfxError::InvalidTarget)),
        };
        let Ok(widget_id) = WidgetId::new(widget) else {
            return Ok(Err(gfx::types::GfxError::InvalidTarget));
        };
        let rect = match self.canvas_widget_rect(window_id, widget_id) {
            Ok(rect) => rect,
            Err(error) => return Ok(Err(error)),
        };
        let scene = match Scene::new(rect.0.max(1.0) as u32, rect.1.max(1.0) as u32) {
            Ok(scene) => scene,
            Err(error) => return Ok(Err(gfx::types::GfxError::Unsupported(error.to_string()))),
        };
        let scene_id = self.next_canvas_id.get();
        self.next_canvas_id.set(scene_id.saturating_add(1));
        self.scenes
            .borrow_mut()
            .insert(scene_id, (window_id, widget_id, scene));
        Ok(Ok(scene_id))
    }

    fn clear(
        &mut self,
        scene: u64,
        sky: gfx::types::Color,
    ) -> wasmtime::Result<Result<(), gfx::types::GfxError>> {
        let mut scenes = self.scenes.borrow_mut();
        let Some((_, _, surface)) = scenes.get_mut(&scene) else {
            return Ok(Err(gfx::types::GfxError::InvalidTarget));
        };
        surface.clear(pack_color(sky.r, sky.g, sky.b, sky.a));
        Ok(Ok(()))
    }

    fn camera(
        &mut self,
        scene: u64,
        eye: Vec<f32>,
        look_at: Vec<f32>,
        fov_degrees: f32,
    ) -> wasmtime::Result<Result<(), gfx::types::GfxError>> {
        // Three floats each, or the call describes nothing. Refused rather
        // than padded: a camera silently placed at the origin is a bug an app
        // author would spend an evening on.
        if eye.len() != 3 || look_at.len() != 3 {
            return Ok(Err(gfx::types::GfxError::Unsupported(
                "camera takes three floats for eye and three for look-at".to_string(),
            )));
        }
        let mut scenes = self.scenes.borrow_mut();
        let Some((_, _, surface)) = scenes.get_mut(&scene) else {
            return Ok(Err(gfx::types::GfxError::InvalidTarget));
        };
        surface.set_camera(
            [eye[0], eye[1], eye[2]],
            [look_at[0], look_at[1], look_at[2]],
            fov_degrees,
        );
        Ok(Ok(()))
    }

    fn light(
        &mut self,
        scene: u64,
        direction: Vec<f32>,
    ) -> wasmtime::Result<Result<(), gfx::types::GfxError>> {
        if direction.len() != 3 {
            return Ok(Err(gfx::types::GfxError::Unsupported(
                "light takes three floats".to_string(),
            )));
        }
        let mut scenes = self.scenes.borrow_mut();
        let Some((_, _, surface)) = scenes.get_mut(&scene) else {
            return Ok(Err(gfx::types::GfxError::InvalidTarget));
        };
        surface.set_light([direction[0], direction[1], direction[2]]);
        Ok(Ok(()))
    }

    fn triangles(
        &mut self,
        scene: u64,
        vertices: Vec<f32>,
        tint: gfx::types::Color,
    ) -> wasmtime::Result<Result<(), gfx::types::GfxError>> {
        let mut scenes = self.scenes.borrow_mut();
        let Some((_, _, surface)) = scenes.get_mut(&scene) else {
            return Ok(Err(gfx::types::GfxError::InvalidTarget));
        };
        surface.triangles(&vertices, (tint.r, tint.g, tint.b, tint.a));
        Ok(Ok(()))
    }

    fn place(
        &mut self,
        scene: u64,
        vertices: Vec<f32>,
        translate: Vec<f32>,
        rotate_degrees: Vec<f32>,
        scale: f32,
        tint: gfx::types::Color,
    ) -> wasmtime::Result<Result<(), gfx::types::GfxError>> {
        if translate.len() != 3 || rotate_degrees.len() != 3 {
            return Ok(Err(gfx::types::GfxError::Unsupported(
                "place takes three floats for translate and three for rotate".to_string(),
            )));
        }
        let mut scenes = self.scenes.borrow_mut();
        let Some((_, _, surface)) = scenes.get_mut(&scene) else {
            return Ok(Err(gfx::types::GfxError::InvalidTarget));
        };
        surface.place(
            &vertices,
            [translate[0], translate[1], translate[2]],
            [rotate_degrees[0], rotate_degrees[1], rotate_degrees[2]],
            scale,
            (tint.r, tint.g, tint.b, tint.a),
        );
        Ok(Ok(()))
    }

    fn upload_texture(
        &mut self,
        scene: u64,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    ) -> wasmtime::Result<Result<u64, gfx::types::GfxError>> {
        let mut scenes = self.scenes.borrow_mut();
        let Some((_, _, surface)) = scenes.get_mut(&scene) else {
            return Ok(Err(gfx::types::GfxError::InvalidTarget));
        };
        match surface.upload_texture(width, height, &rgba) {
            Ok(handle) => Ok(Ok(handle)),
            Err(error) => Ok(Err(gfx::types::GfxError::Unsupported(error.to_string()))),
        }
    }

    fn textured(
        &mut self,
        scene: u64,
        vertices: Vec<f32>,
        uvs: Vec<f32>,
        texture: u64,
        tint: gfx::types::Color,
    ) -> wasmtime::Result<Result<(), gfx::types::GfxError>> {
        let mut scenes = self.scenes.borrow_mut();
        let Some((_, _, surface)) = scenes.get_mut(&scene) else {
            return Ok(Err(gfx::types::GfxError::InvalidTarget));
        };
        surface.textured(&vertices, &uvs, texture, (tint.r, tint.g, tint.b, tint.a));
        Ok(Ok(()))
    }

    fn cull_back_faces(
        &mut self,
        scene: u64,
        enabled: bool,
    ) -> wasmtime::Result<Result<(), gfx::types::GfxError>> {
        let mut scenes = self.scenes.borrow_mut();
        let Some((_, _, surface)) = scenes.get_mut(&scene) else {
            return Ok(Err(gfx::types::GfxError::InvalidTarget));
        };
        surface.set_cull_back_faces(enabled);
        Ok(Ok(()))
    }

    fn present(&mut self, scene: u64) -> wasmtime::Result<Result<(), gfx::types::GfxError>> {
        let (window, widget, image) = {
            // Mutable because presenting is what fills the frame: triangles
            // are queued as they are drawn and rasterized here, once, across
            // every core.
            let mut scenes = self.scenes.borrow_mut();
            let Some((window, widget, surface)) = scenes.get_mut(&scene) else {
                return Ok(Err(gfx::types::GfxError::InvalidTarget));
            };
            let image = match surface.render_image() {
                Ok(image) => image,
                Err(error) => return Ok(Err(gfx::types::GfxError::Platform(error.to_string()))),
            };
            (*window, *widget, image)
        };
        self.images
            .borrow_mut()
            .insert((window, widget), std::sync::Arc::new(image));
        let result = self
            .sync_native_widgets(window)
            .map_err(|error| gfx::types::GfxError::Platform(error.to_string()));
        // A presented scene is a real drawn 3D frame -- capture here so a game
        // that steers with key-held and never calls wait still gets shot with
        // its scene on screen, not the blank frame before the first present.
        if result.is_ok() {
            self.maybe_take_screenshot();
        }
        Ok(result)
    }
}

/// Write an `0xAARRGGBB` framebuffer to a PNG file as RGBA.
fn write_argb_png(
    buffer: &[u32],
    width: u32,
    height: u32,
    path: &std::path::Path,
) -> std::io::Result<()> {
    let mut rgba = Vec::with_capacity(buffer.len() * 4);
    for px in buffer {
        rgba.push(((px >> 16) & 0xFF) as u8);
        rgba.push(((px >> 8) & 0xFF) as u8);
        rgba.push((px & 0xFF) as u8);
        rgba.push(((px >> 24) & 0xFF) as u8);
    }
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut encoder = png::Encoder::new(file, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder
        .write_header()
        .map_err(|err| std::io::Error::other(err.to_string()))?;
    writer
        .write_image_data(&rgba)
        .map_err(|err| std::io::Error::other(err.to_string()))?;
    Ok(())
}

fn audio_permission_denied() -> audio::types::AudioError {
    audio::types::AudioError::PermissionDenied
}

fn capture_config(config: audio::types::StreamConfig) -> CaptureConfig {
    CaptureConfig {
        sample_rate: config.sample_rate,
        channels: config.channels,
        format: match config.format {
            audio::types::SampleFormat::PcmS16 => CaptureSampleFormat::PcmS16,
            audio::types::SampleFormat::Float32 => CaptureSampleFormat::Float32,
        },
        buffer_frames: config.buffer_frames,
    }
}

fn capture_error(error: CaptureError) -> audio::types::AudioError {
    match error {
        CaptureError::InvalidStream => audio::types::AudioError::InvalidStream,
        CaptureError::DeviceUnavailable => audio::types::AudioError::DeviceUnavailable,
        CaptureError::InvalidConfig(message) => audio::types::AudioError::Unsupported(message),
        CaptureError::Platform(message) => audio::types::AudioError::Platform(message),
    }
}

fn playback_config(config: audio::types::StreamConfig) -> PlaybackConfig {
    PlaybackConfig {
        sample_rate: config.sample_rate,
        channels: config.channels,
        format: match config.format {
            audio::types::SampleFormat::PcmS16 => PlaybackSampleFormat::PcmS16,
            audio::types::SampleFormat::Float32 => PlaybackSampleFormat::Float32,
        },
        buffer_frames: config.buffer_frames,
    }
}

fn playback_error(error: PlaybackError) -> audio::types::AudioError {
    match error {
        PlaybackError::InvalidStream => audio::types::AudioError::InvalidStream,
        PlaybackError::DeviceUnavailable => audio::types::AudioError::DeviceUnavailable,
        PlaybackError::InvalidConfig(message) => audio::types::AudioError::Unsupported(message),
        PlaybackError::Platform(message) => audio::types::AudioError::Platform(message),
    }
}

impl audio::types::Host for Phase3GuiHost {}

impl audio::playback::Host for Phase3GuiHost {
    fn open(
        &mut self,
        config: audio::types::StreamConfig,
    ) -> wasmtime::Result<Result<u64, audio::types::AudioError>> {
        if self
            .runtime
            .guard()
            .check(&UapiCall::Audio(AudioCall::Playback))
            .is_err()
        {
            return Ok(Err(audio_permission_denied()));
        }
        Ok(self
            .audio_playback
            .open(playback_config(config))
            .map_err(playback_error))
    }

    fn start(&mut self, stream_id: u64) -> wasmtime::Result<Result<(), audio::types::AudioError>> {
        if self
            .runtime
            .guard()
            .check(&UapiCall::Audio(AudioCall::Playback))
            .is_err()
        {
            return Ok(Err(audio_permission_denied()));
        }
        Ok(self.audio_playback.start(stream_id).map_err(playback_error))
    }

    fn stop(&mut self, stream_id: u64) -> wasmtime::Result<Result<(), audio::types::AudioError>> {
        if self
            .runtime
            .guard()
            .check(&UapiCall::Audio(AudioCall::Playback))
            .is_err()
        {
            return Ok(Err(audio_permission_denied()));
        }
        Ok(self.audio_playback.stop(stream_id).map_err(playback_error))
    }

    fn load_sound(
        &mut self,
        stream_id: u64,
        bytes: Vec<u8>,
    ) -> wasmtime::Result<Result<u64, audio::types::AudioError>> {
        if self
            .runtime
            .guard()
            .check(&UapiCall::Audio(AudioCall::Playback))
            .is_err()
        {
            return Ok(Err(audio_permission_denied()));
        }
        Ok(self
            .audio_playback
            .load_sound(stream_id, &bytes)
            .map_err(playback_error))
    }

    fn play_sound(
        &mut self,
        stream_id: u64,
        sound: u64,
        gain: f32,
    ) -> wasmtime::Result<Result<(), audio::types::AudioError>> {
        if self
            .runtime
            .guard()
            .check(&UapiCall::Audio(AudioCall::Playback))
            .is_err()
        {
            return Ok(Err(audio_permission_denied()));
        }
        Ok(self
            .audio_playback
            .play_sound(stream_id, sound, gain)
            .map_err(playback_error))
    }

    fn stop_sound(
        &mut self,
        stream_id: u64,
        sound: u64,
    ) -> wasmtime::Result<Result<(), audio::types::AudioError>> {
        if self
            .runtime
            .guard()
            .check(&UapiCall::Audio(AudioCall::Playback))
            .is_err()
        {
            return Ok(Err(audio_permission_denied()));
        }
        Ok(self
            .audio_playback
            .stop_sound(stream_id, sound)
            .map_err(playback_error))
    }

    fn write(
        &mut self,
        stream_id: u64,
        bytes: Vec<u8>,
    ) -> wasmtime::Result<Result<u32, audio::types::AudioError>> {
        if self
            .runtime
            .guard()
            .check(&UapiCall::Audio(AudioCall::Playback))
            .is_err()
        {
            return Ok(Err(audio_permission_denied()));
        }
        Ok(self
            .audio_playback
            .write(stream_id, &bytes)
            .map_err(playback_error))
    }
}

impl audio::capture::Host for Phase3GuiHost {
    fn open(
        &mut self,
        config: audio::types::StreamConfig,
    ) -> wasmtime::Result<Result<u64, audio::types::AudioError>> {
        if self
            .runtime
            .guard()
            .check(&UapiCall::Audio(AudioCall::Capture))
            .is_err()
        {
            return Ok(Err(audio_permission_denied()));
        }
        Ok(self
            .audio_capture
            .open(capture_config(config))
            .map_err(capture_error))
    }

    fn start(&mut self, stream_id: u64) -> wasmtime::Result<Result<(), audio::types::AudioError>> {
        if self
            .runtime
            .guard()
            .check(&UapiCall::Audio(AudioCall::Capture))
            .is_err()
        {
            return Ok(Err(audio_permission_denied()));
        }
        Ok(self.audio_capture.start(stream_id).map_err(capture_error))
    }

    fn stop(&mut self, stream_id: u64) -> wasmtime::Result<Result<(), audio::types::AudioError>> {
        if self
            .runtime
            .guard()
            .check(&UapiCall::Audio(AudioCall::Capture))
            .is_err()
        {
            return Ok(Err(audio_permission_denied()));
        }
        Ok(self.audio_capture.stop(stream_id).map_err(capture_error))
    }

    fn read(
        &mut self,
        stream_id: u64,
        max_bytes: u32,
    ) -> wasmtime::Result<Result<Vec<u8>, audio::types::AudioError>> {
        if self
            .runtime
            .guard()
            .check(&UapiCall::Audio(AudioCall::Capture))
            .is_err()
        {
            return Ok(Err(audio_permission_denied()));
        }
        Ok(self
            .audio_capture
            .read(stream_id, max_bytes)
            .map_err(capture_error))
    }
}

impl speech::transcription::Host for Phase3GuiHost {
    fn transcribe(
        &mut self,
        model_asset: String,
        pcm_s16_le: Vec<u8>,
        sample_rate: u32,
        language: Option<String>,
    ) -> wasmtime::Result<
        Result<speech::transcription::Transcript, speech::transcription::SpeechError>,
    > {
        Ok(self
            .speech
            .transcribe(&model_asset, &pcm_s16_le, sample_rate, language.as_deref())
            .map(|text| speech::transcription::Transcript { text })
            .map_err(speech_error))
    }

    fn match_line(
        &mut self,
        model_asset: String,
        pcm_s16_le: Vec<u8>,
        sample_rate: u32,
        language: Option<String>,
        expected: String,
    ) -> wasmtime::Result<Result<u8, speech::transcription::MatchError>> {
        Ok(self
            .speech
            .match_line(
                &model_asset,
                &pcm_s16_le,
                sample_rate,
                language.as_deref(),
                &expected,
            )
            .map_err(match_error))
    }

    fn match_line_stream(
        &mut self,
        model_asset: String,
        pcm_s16_le: Vec<u8>,
        sample_rate: u32,
        language: Option<String>,
        expected: String,
        finish: bool,
    ) -> wasmtime::Result<Result<Option<u8>, speech::transcription::MatchError>> {
        Ok(self
            .speech
            .match_line_stream(
                &model_asset,
                &pcm_s16_le,
                sample_rate,
                language.as_deref(),
                &expected,
                finish,
            )
            .map_err(match_error))
    }
}

fn speech_error(error: SpeechError) -> speech::transcription::SpeechError {
    match error {
        SpeechError::InvalidRequest(message) => {
            speech::transcription::SpeechError::InvalidRequest(message)
        }
        SpeechError::ModelNotFound => speech::transcription::SpeechError::ModelNotFound,
        SpeechError::ModelInvalid(message) => {
            speech::transcription::SpeechError::ModelInvalid(message)
        }
        SpeechError::Unsupported(message) => {
            speech::transcription::SpeechError::Unsupported(message)
        }
        SpeechError::Inference(message) => speech::transcription::SpeechError::Inference(message),
    }
}

fn match_error(error: SpeechError) -> speech::transcription::MatchError {
    match error {
        SpeechError::InvalidRequest(_) => speech::transcription::MatchError::InvalidRequest,
        SpeechError::ModelNotFound => speech::transcription::MatchError::ModelNotFound,
        SpeechError::ModelInvalid(_) => speech::transcription::MatchError::ModelInvalid,
        SpeechError::Unsupported(_) => speech::transcription::MatchError::Unsupported,
        SpeechError::Inference(_) => speech::transcription::MatchError::Inference,
    }
}

/// Ask the operating system to show its open-file dialog.
///
/// One implementation for all three systems, through `rfd`, which uses the
/// native dialog on each: NSOpenPanel on macOS, the common item dialog on
/// Windows, and the XDG desktop portal on Linux. A picker that worked on the
/// machine an app was built on and failed when it was shared would be the exact
/// failure Krate exists to remove, so there is deliberately no per-platform
/// branch here to drift.
///
/// `filter` is a comma-separated extension list. It narrows what the dialog
/// offers and is not a rule the runtime enforces -- whatever the person picks
/// is what the app gets, because the click is the grant.
fn choose_file_on_host(title: &str, filter: &str) -> Result<Option<std::path::PathBuf>, String> {
    let mut dialog = rfd::FileDialog::new();
    if !title.is_empty() {
        dialog = dialog.set_title(title);
    }

    let extensions: Vec<&str> = filter
        .split(',')
        .map(str::trim)
        .filter(|ext| !ext.is_empty())
        .collect();
    if !extensions.is_empty() {
        dialog = dialog.add_filter("Supported files", &extensions);
    }

    // `None` is a cancelled dialog, which is a normal answer rather than a
    // failure, and the caller reports it as such.
    Ok(dialog.pick_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scroll_offset_clamps_to_content_extent() {
        // Content 192 tall in a 120 viewport: max offset 72.
        assert_eq!(clamped_scroll_offset(0.0, 30.0, 192.0, 120.0), 30.0);
        assert_eq!(clamped_scroll_offset(60.0, 30.0, 192.0, 120.0), 72.0);
        assert_eq!(clamped_scroll_offset(10.0, -30.0, 192.0, 120.0), 0.0);
        // Content shorter than the viewport never scrolls.
        assert_eq!(clamped_scroll_offset(0.0, 30.0, 80.0, 120.0), 0.0);
    }

    fn headless_host() -> Phase3GuiHost {
        Phase3GuiHost::new(
            UapiGuard::new(Default::default()),
            Phase3HostUiMode::HeadlessDraft,
        )
        .expect("headless host")
    }

    fn capture_config() -> audio::types::StreamConfig {
        audio::types::StreamConfig {
            sample_rate: 16_000,
            channels: 1,
            format: audio::types::SampleFormat::PcmS16,
            buffer_frames: 1_600,
        }
    }

    #[test]
    fn microphone_open_denies_before_reaching_the_audio_adapter() {
        let mut host = headless_host();
        let result = <Phase3GuiHost as audio::capture::Host>::open(&mut host, capture_config())
            .expect("host call");

        assert!(matches!(
            result,
            Err(audio::types::AudioError::PermissionDenied)
        ));
    }

    #[test]
    fn granted_microphone_open_reaches_capture_validation() {
        let policy = krate_policy::SessionPolicy::from_cli_grants(&["audio.capture".to_string()])
            .expect("capture policy");
        let mut host = Phase3GuiHost::new(UapiGuard::new(policy), Phase3HostUiMode::HeadlessDraft)
            .expect("headless host");
        let result = <Phase3GuiHost as audio::capture::Host>::open(
            &mut host,
            audio::types::StreamConfig {
                sample_rate: 0,
                ..capture_config()
            },
        )
        .expect("host call");

        assert!(matches!(
            result,
            Err(audio::types::AudioError::Unsupported(_))
        ));
    }

    #[test]
    fn headless_waits_report_a_close_once_idle() {
        let host = headless_host();
        // The app gets a grace period to reach its event loop...
        for _ in 1..HEADLESS_IDLE_WAIT_LIMIT {
            assert!(host.headless_close_request().is_none());
        }
        // ...and then is told the window closed, so its loop can end instead of
        // spinning out a wait budget that nothing will ever interrupt.
        assert!(matches!(
            host.headless_close_request(),
            Some(ui::types::Event::CloseRequested(_))
        ));
    }

    #[test]
    fn a_real_event_resets_the_headless_idle_count() {
        let host = headless_host();
        for _ in 0..(HEADLESS_IDLE_WAIT_LIMIT - 1) {
            assert!(host.headless_close_request().is_none());
        }
        // An app that is doing work keeps its window: the counter only counts
        // *consecutive* empty waits, and `wait` clears it on every real event.
        host.idle_waits.set(0);
        assert!(host.headless_close_request().is_none());
    }

    #[test]
    fn a_wait_with_a_timeout_is_left_to_time_out_on_its_own() {
        // Only an unbounded wait gets the synthetic close. A guest that asked
        // for a timeout is handed control back with no event, because "the
        // window closed" states that a person closed it -- krate-hello-gui
        // reports exactly that as exit 2 -- and on a quiet round nobody did.
        let mut host = headless_host();
        for _ in 0..(HEADLESS_IDLE_WAIT_LIMIT * 3) {
            let event = ui::events::Host::wait(&mut host, Some(0)).expect("bounded wait");
            assert!(
                event.is_none(),
                "a bounded wait must report no event, never a close nobody asked for"
            );
        }
    }

    #[test]
    fn a_windowed_host_is_never_closed_by_the_idle_rule() {
        let mut host = headless_host();
        host.headless = false;
        for _ in 0..(HEADLESS_IDLE_WAIT_LIMIT * 4) {
            assert!(
                host.headless_close_request().is_none(),
                "a host with a real window must stay open until the person closes it"
            );
        }
    }

    #[test]
    fn presses_focus_text_entry_widgets_only() {
        assert!(press_focuses(WidgetKind::TextField));
        assert!(press_focuses(WidgetKind::TextArea));
        assert!(!press_focuses(WidgetKind::Button));
        assert!(!press_focuses(WidgetKind::Text));
        assert!(!press_focuses(WidgetKind::Stack));
    }

    fn wit_node(kind: ui::types::WidgetKind, cursor: Option<(u32, u32)>) -> ui::types::WidgetNode {
        ui::types::WidgetNode {
            id: 1,
            parent: None,
            kind,
            label: Some("hello".to_string()),
            role: None,
            style: ui::types::Style {
                width: Some(100.0),
                height: Some(30.0),
                grow: 0.0,
                padding: 0.0,
            },
            checked: None,
            value: None,
            selected: None,
            text_cursor: cursor.map(|(c, a)| ui::types::TextCursor {
                cursor: c,
                anchor: a,
            }),
        }
    }

    #[test]
    fn a_text_caret_lowers_onto_a_text_widget() {
        let node = widget_node_from_wit(wit_node(ui::types::WidgetKind::TextArea, Some((2, 0))))
            .expect("a text area may carry a caret");
        assert_eq!(node.text_cursor, Some((2, 0)));
    }

    /// A window with one image widget as its root, ready to be given a picture.
    fn host_with_image_widget() -> (Phase3GuiHost, u64, u64) {
        use ui::window::Host as _;
        let mut host = headless_host();
        let window = host
            .create(
                "viewer".to_string(),
                ui::types::WindowSize {
                    width: 200,
                    height: 200,
                },
            )
            .expect("create call")
            .expect("a window");

        let mut node = wit_node(ui::types::WidgetKind::Image, None);
        node.label = None;
        let widget = node.id;
        ui::tree::Host::set_root(&mut host, window, node)
            .expect("set_root call")
            .expect("an image widget may be the root");
        (host, window, widget)
    }

    /// A window with one canvas widget as its root.
    fn host_with_canvas_widget() -> (Phase3GuiHost, u64, u64) {
        use ui::window::Host as _;
        let mut host = headless_host();
        let window = host
            .create(
                "sketch".to_string(),
                ui::types::WindowSize {
                    width: 200,
                    height: 200,
                },
            )
            .expect("create call")
            .expect("a window");
        let node = wit_node(ui::types::WidgetKind::Canvas, None);
        let widget = node.id;
        ui::tree::Host::set_root(&mut host, window, node)
            .expect("set_root call")
            .expect("a canvas may be the root");
        (host, window, widget)
    }

    #[test]
    fn reading_a_key_does_not_swallow_the_close_request() {
        // The bug that made the window's close button do nothing.
        //
        // `key-held` pumps the platform queue so a game that never calls
        // `poll` still reads live input. It used to discard whatever the pump
        // returned. A game reading ten keys a frame therefore pumped ten
        // times, and a CloseRequested that surfaced during any of them was
        // destroyed before the game's own `poll` could match on it -- so the
        // app kept running and the person clicking X saw nothing happen.
        let (mut host, window, _widget) = host_with_canvas_widget();

        // A close request arrives, then the app reads a key -- the order a
        // game actually produces at the top of its frame.
        host.pending_events
            .borrow_mut()
            .push_back(ui::types::Event::CloseRequested(window));
        let _ =
            ui::events::Host::key_held(&mut host, "ArrowLeft".to_string()).expect("key-held call");

        // The close must still be there to be delivered.
        let event = ui::events::Host::poll(&mut host).expect("poll call");
        assert!(
            matches!(event, Some(ui::types::Event::CloseRequested(id)) if id == window),
            "the close request must survive a key-held call, got {event:?}"
        );
    }

    #[test]
    fn a_held_key_is_forgotten_when_the_window_loses_focus() {
        // The bug this whole call exists to prevent. An app tracking presses
        // itself never sees the release once focus moves away, so the player
        // who alt-tabs mid-stride comes back to a character still running.
        let mut host = headless_host();
        host.held_keys.borrow_mut().insert("KeyW".to_string());
        assert!(ui::events::Host::key_held(&mut host, "KeyW".to_string()).expect("query"));

        // Losing focus clears everything; gaining it does not.
        host.held_keys.borrow_mut().insert("KeyW".to_string());
        host.on_window_focus_changed(false);
        assert!(
            !ui::events::Host::key_held(&mut host, "KeyW".to_string()).expect("query"),
            "a key held when focus was lost must not stay held"
        );

        host.held_keys.borrow_mut().insert("KeyA".to_string());
        host.on_window_focus_changed(true);
        assert!(
            ui::events::Host::key_held(&mut host, "KeyA".to_string()).expect("query"),
            "gaining focus must not clear keys the person is holding"
        );

        // A key nobody pressed is not held.
        assert!(!ui::events::Host::key_held(&mut host, "KeyZ".to_string()).expect("query"));
    }

    #[test]
    fn a_headless_animation_loop_cannot_run_forever() {
        // The demo path: someone is sent a .krate and opens it with no
        // arguments. An animated app calls request-redraw every frame and
        // immediately receives that redraw back, so the event queue is never
        // empty and every "is it idle" check says no. Before the wall-clock
        // budget, that was a terminal frozen for as long as the person waited.
        let mut host = headless_host();
        use ui::window::Host as _;
        let window = host
            .create(
                "loop".to_string(),
                ui::types::WindowSize {
                    width: 100,
                    height: 100,
                },
            )
            .expect("create call")
            .expect("a window");

        // First wait starts the clock rather than ending the run: an app is
        // entitled to its budget, not merely to one call.
        let first = ui::events::Host::wait(&mut host, Some(1)).expect("wait call");
        assert!(
            !matches!(first, Some(ui::types::Event::CloseRequested(_))),
            "the budget must not fire on the very first wait"
        );

        // Pretend the run began long ago, exactly as a real one would after
        // five seconds of frames.
        host.headless_started.set(Some(
            std::time::Instant::now() - HEADLESS_RUN_BUDGET - std::time::Duration::from_millis(1),
        ));

        // Now feed the queue the way an animation loop does, then wait. The
        // close must win over the app's own redraw.
        let _ = ui::window::Host::request_redraw(&mut host, window);
        let event = ui::events::Host::wait(&mut host, Some(16)).expect("wait call");
        assert!(
            matches!(event, Some(ui::types::Event::CloseRequested(_))),
            "a spent budget must end the run even with events queued: {event:?}"
        );
    }

    #[test]
    fn canvas_drawing_reaches_the_widget_only_at_present() {
        // The whole gfx.canvas2d path: bind, draw, present -- and the raster
        // lands in the same per-widget image store every host already reads.
        // Present is the only publisher: a hundred fills must not re-render
        // the window a hundred times, so before it the store stays empty.
        let (mut host, window, widget) = host_with_canvas_widget();
        let canvas = gfx::canvas2d::Host::bind(&mut host, window, widget)
            .expect("bind call")
            .expect("a canvas widget binds");

        gfx::canvas2d::Host::clear(
            &mut host,
            canvas,
            gfx::types::Color {
                r: 0.0,
                g: 0.0,
                b: 1.0,
                a: 1.0,
            },
        )
        .expect("clear call")
        .expect("clear succeeds");
        gfx::canvas2d::Host::fill_rect(
            &mut host,
            canvas,
            gfx::types::Rect {
                x: 0.0,
                y: 0.0,
                width: 10.0,
                height: 10.0,
            },
            gfx::types::Color {
                r: 1.0,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            },
        )
        .expect("fill call")
        .expect("fill succeeds");

        let id = host.window_id(window).expect("window");
        let widget_id = WidgetId::new(widget).expect("widget");
        assert!(
            host.images.borrow().get(&(id, widget_id)).is_none(),
            "nothing may reach the widget before present"
        );

        gfx::canvas2d::Host::present(&mut host, canvas)
            .expect("present call")
            .expect("present succeeds");

        let images = host.images.borrow();
        let image = images
            .get(&(id, widget_id))
            .expect("present publishes the raster");
        assert_eq!(&image.rgba[0..4], &[255, 0, 0, 255], "the red fill");
        let last = image.rgba.len() - 4;
        assert_eq!(&image.rgba[last..], &[0, 0, 255, 255], "the blue clear");
    }

    #[test]
    fn presenting_a_canvas_writes_the_requested_screenshot() {
        // The `krate shoot` path end to end: a headless host asked for a
        // screenshot paints the window to a real PNG when the app presents a
        // frame, and the PNG decodes to the window's pixel size. This is the
        // tool every app test relies on to see what an app draws.
        let dir = std::env::temp_dir().join(format!("krate-shoot-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("frame.png");

        let (mut host, window, widget) = host_with_canvas_widget();
        host.screenshot = Some((path.clone(), 2.0));
        let canvas = gfx::canvas2d::Host::bind(&mut host, window, widget)
            .expect("bind call")
            .expect("a canvas widget binds");
        gfx::canvas2d::Host::clear(
            &mut host,
            canvas,
            gfx::types::Color {
                r: 1.0,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            },
        )
        .expect("clear call")
        .expect("clear succeeds");
        gfx::canvas2d::Host::present(&mut host, canvas)
            .expect("present call")
            .expect("present succeeds");

        assert!(host.screenshot_taken.get(), "the screenshot was taken");
        let bytes = std::fs::read(&path).expect("screenshot file exists");
        assert!(!bytes.is_empty(), "the screenshot is not empty");
        // Decode it: a valid PNG at the window size times the 2x scale.
        let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
        let reader = decoder.read_info().expect("valid png");
        let info = reader.info();
        assert_eq!(
            (info.width, info.height),
            (400, 400),
            "200x200 window at 2x"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_canvas_refuses_to_bind_to_a_button() {
        use ui::window::Host as _;
        let mut host = headless_host();
        let window = host
            .create(
                "app".to_string(),
                ui::types::WindowSize {
                    width: 100,
                    height: 100,
                },
            )
            .expect("create call")
            .expect("a window");
        let node = wit_node(ui::types::WidgetKind::Button, None);
        let widget = node.id;
        ui::tree::Host::set_root(&mut host, window, node)
            .expect("set_root call")
            .expect("a button root");

        let err = gfx::canvas2d::Host::bind(&mut host, window, widget)
            .expect("bind call")
            .expect_err("a button is not a canvas");
        assert!(matches!(err, gfx::types::GfxError::Unsupported(_)));
    }

    #[test]
    fn a_picture_reaches_the_widget_it_was_sent_for() {
        // The whole path: an app sets pixels through krate:ui/image, and they
        // arrive on the placement the painters draw from. Without this, a
        // picture could be accepted and stored and never reach a window.
        let (mut host, window, widget) = host_with_image_widget();
        ui::image::Host::set_pixels(
            &mut host,
            window,
            widget,
            ui::image::ImagePixels {
                width: 2,
                height: 2,
                rgba: vec![255u8; 16],
            },
        )
        .expect("set_pixels call")
        .expect("an image widget accepts a picture");

        let id = host.window_id(window).expect("the window exists");
        let widget_id = WidgetId::new(widget).expect("a real widget id");
        let stored = host.images.borrow();
        let picture = stored
            .get(&(id, widget_id))
            .expect("the picture must be held for this widget");
        assert_eq!((picture.width, picture.height), (2, 2));
        drop(stored);

        // And clearing takes it away, leaving the empty frame a viewer shows
        // before anybody has chosen a file.
        ui::image::Host::clear(&mut host, window, widget)
            .expect("clear call")
            .expect("clearing succeeds");
        assert!(host.images.borrow().is_empty());
    }

    #[test]
    fn a_picture_is_refused_for_a_widget_that_cannot_show_one() {
        // Storing pixels for a button would leave the app believing it had
        // shown something while nothing ever drew.
        use ui::window::Host as _;
        let mut host = headless_host();
        let window = host
            .create(
                "app".to_string(),
                ui::types::WindowSize {
                    width: 100,
                    height: 100,
                },
            )
            .expect("create call")
            .expect("a window");
        let node = wit_node(ui::types::WidgetKind::Button, None);
        let widget = node.id;
        ui::tree::Host::set_root(&mut host, window, node)
            .expect("set_root call")
            .expect("a button root");

        let err = ui::image::Host::set_pixels(
            &mut host,
            window,
            widget,
            ui::image::ImagePixels {
                width: 1,
                height: 1,
                rgba: vec![0u8; 4],
            },
        )
        .expect("set_pixels call")
        .expect_err("a button cannot show a picture");
        assert!(matches!(err, ui::types::UiError::Unsupported(_)));
    }

    #[test]
    fn a_picture_whose_bytes_do_not_match_its_size_is_refused() {
        // Every host indexes this buffer by row and column. A buffer shorter
        // than its stated size would read past the end on the last row, and
        // the guest is the untrusted side of this boundary.
        let (mut host, window, widget) = host_with_image_widget();
        let err = ui::image::Host::set_pixels(
            &mut host,
            window,
            widget,
            ui::image::ImagePixels {
                width: 4,
                height: 4,
                // A 4x4 image needs 64 bytes, not 8.
                rgba: vec![0u8; 8],
            },
        )
        .expect("set_pixels call")
        .expect_err("the byte count must match the size");
        assert!(matches!(err, ui::types::UiError::Unsupported(_)));
        assert!(host.images.borrow().is_empty(), "nothing may be stored");
    }

    #[test]
    fn a_text_caret_on_a_non_text_widget_is_rejected() {
        // A caret only means something on editable text; carrying one on, say,
        // a button is a guest bug and must not silently pass through.
        let err = widget_node_from_wit(wit_node(ui::types::WidgetKind::Button, Some((1, 1))))
            .expect_err("a button cannot carry a text caret");
        assert!(matches!(err, ui::types::UiError::Unsupported(_)));
    }
}
