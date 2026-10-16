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

use krate_adapter_common::ui::{
    ImagePixels, Modifiers, PointerButton, Theme, UiAdapterError, UiEvent, WidgetId, WidgetKind,
    WidgetNode, WidgetPlacement, WidgetStyle, WindowBackendKind, WindowId, WindowOptions,
    WindowSize,
};
use krate_layout::{absolute_rect, LayoutViewport};
use std::sync::Arc;

use crate::{
    audio_capture::{AudioCaptureRuntime, CaptureConfig, CaptureError, CaptureSampleFormat},
    audio_playback::{AudioPlaybackRuntime, PlaybackConfig, PlaybackError, PlaybackSampleFormat},
    camera_capture::{
        CameraCaptureRuntime, CameraConfig, CameraError, FrameFormat as CameraFrameFormat,
    },
    canvas_raster::{pack_color, CanvasSurface},
    phase3_gui_bindings::krate::{audio, camera, gfx, speech, ui},
    phase3_ui::{Phase3HostUiMode, Phase3UiDispatcher, Phase3UiRuntime, UiDispatchError},
    scene3d::SceneBackend,
    speech_synthesis::{SayError, SpeechSynthesis},
    speech_transcription::{LocalSpeechRuntime, SpeechError},
    uapi::{AudioCall, CameraCall, UapiCall, UapiGuard, UiCall},
};

// krate:gpu lives beside the host it extends, as a child module so it can
// present through the same private paths a scene3d frame takes.
#[path = "gpu_host.rs"]
mod gpu_host;

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

/// The headless budget for this run: [`HEADLESS_RUN_BUDGET`], or
/// `KRATE_HEADLESS_SECS` when set. The knob is for measuring an app whose
/// start-up alone outlasts five seconds (a GPU engine loading a large scene)
/// without a window; like `KRATE_GPU_UNPACED`, nothing a person runs sets it.
fn headless_run_budget() -> std::time::Duration {
    headless_secs_asked().unwrap_or(HEADLESS_RUN_BUDGET)
}

fn headless_secs_asked() -> Option<std::time::Duration> {
    std::env::var("KRATE_HEADLESS_SECS")
        .ok()
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|secs| secs.is_finite() && *secs > 0.0)
        .map(std::time::Duration::from_secs_f64)
}

/// How long a headless app has to leave after being told the window closed,
/// before the runtime ends the process for it.
///
/// Generous on purpose: the point is to bound the run, not to race a shutdown
/// that is genuinely in progress. An app that saves on the way out has two
/// full seconds of frames to do it in.
const HEADLESS_CLOSE_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

/// A headless window's pointer: whether the app holds it, and the raw motion
/// not yet taken.
type HeldPointer = (bool, (f32, f32));

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
    /// Windows a driven headless run holds the pointer in, and the raw motion
    /// waiting to be taken (IC-937): the run's `look` step moves it, so a
    /// script can turn a game's camera with no screen at all.
    headless_pointer: std::cell::RefCell<std::collections::BTreeMap<u64, HeldPointer>>,
    /// Consecutive `wait` calls that timed out with no event, used only when
    /// [`headless`] is set. Once this passes [`HEADLESS_IDLE_WAIT_LIMIT`] the
    /// host synthesises a close request so the guest exits the way it would if
    /// a person had closed the window.
    idle_waits: std::cell::Cell<u32>,
    /// When the last frame was published, for pacing the next one.
    last_present: std::cell::Cell<Option<std::time::Instant>>,
    /// Per-frame publish timings (copy ms, sync ms), kept only when
    /// KRATE_FRAME_STATS=1 and reported every 60 frames. The scroll-feel
    /// debugging rule: never guess which half of the frame is slow.
    publish_times: std::cell::RefCell<Vec<(f32, f32)>>,
    /// Per-present loop timings (gap since last present entry, time slept
    /// for pacing), same gate and cadence. When frames are slow and the
    /// work is fast, the loop itself is the suspect.
    present_times: std::cell::RefCell<Vec<(f32, f32)>>,
    last_present_entry: std::cell::Cell<Option<std::time::Instant>>,
    /// When the native event queue was last drained, for coalescing the
    /// several pumps a frame loop asks for into one.
    last_pump: std::cell::Cell<Option<std::time::Instant>>,
    /// How many times an interrupt has been turned into a close request.
    interrupts: std::cell::Cell<u32>,
    /// See [`Config::app_name`](crate::Config::app_name).
    app_name: Option<String>,
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
    /// Drops and hovers, already turned into what a guest may see.
    ///
    /// Phase 3's `event` variant has no case for either, and its WIT is
    /// frozen, so these cannot ride the ordinary queue -- they would have
    /// nowhere to go. Phase 4's events implementation drains this first.
    ///
    /// A drop is stored as a NAME and a TOKEN, never a path: the conversion
    /// happens here, at the moment the host learns of it, so there is no point
    /// later where a path could reach a guest by accident (K-175).
    pending_drops: std::cell::RefCell<std::collections::VecDeque<DropNotice>>,
    /// Whether `KRATE_TEST_DROP` has already been delivered, so a test drop
    /// arrives once rather than on every poll.
    test_drop_done: std::cell::Cell<bool>,
    /// When this headless run began waiting for events, for [`HEADLESS_RUN_BUDGET`].
    /// Set on the first wait rather than at construction, so time spent
    /// building the window tree is not charged against the budget.
    headless_started: std::cell::Cell<Option<std::time::Instant>>,
    /// When the headless budget told the guest its window closed, for
    /// [`HEADLESS_CLOSE_GRACE`].
    headless_close_told: std::cell::Cell<Option<std::time::Instant>>,
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
    /// Pictures canvases keep (krate:gfx/canvas-images), by (canvas, handle),
    /// and the next handle. One Arc per picture, so every frame that draws
    /// it hands the adapters the same picture and they upload it once.
    kept_images: std::cell::RefCell<std::collections::BTreeMap<(u64, u64), Arc<ImagePixels>>>,
    /// The videos this run opened (krate:gfx/video, IC-922) and the frame
    /// each last showed, so a draw between new frames hands the adapters the
    /// same picture and they upload it once.
    pub(crate) videos: std::cell::RefCell<crate::video::Videos>,
    pub(crate) video_frames: std::cell::RefCell<std::collections::BTreeMap<u64, Arc<ImagePixels>>>,
    /// Each video's state as last read, and when (read at most once a frame).
    pub(crate) video_states: std::cell::RefCell<
        std::collections::BTreeMap<u64, (std::time::Instant, crate::video::State)>,
    >,
    /// Where the app's bundled assets are, for a video named by path.
    pub(crate) asset_root: Option<std::path::PathBuf>,
    /// The web pages this run opened (IC-927): their windows, and headless,
    /// what each shows.
    pub(crate) web_pages: std::cell::RefCell<
        std::collections::BTreeMap<u64, (u64, krate_adapter_common::ui::WebPageState)>,
    >,
    next_web_page: std::cell::Cell<u64>,
    /// What each window last told the screen reader (IC-926), for
    /// check-app and `KRATE_ACCESS_DUMP`.
    pub(crate) described: std::cell::RefCell<
        std::collections::BTreeMap<u64, Vec<krate_adapter_common::ui::AccessElement>>,
    >,
    /// The look the platform reported last, and when (IC-925).
    appearance_cache:
        std::cell::Cell<Option<(std::time::Instant, krate_adapter_common::ui::Appearance)>>,
    /// The size each window was asked for, and the canvases whose apps have
    /// asked how big they are (`canvas-size`). A canvas app that never asks
    /// can only be drawing for the size it requested; where the window is
    /// another size (a phone, a resized window) the host fits that size to
    /// it, as if the app had set it as its design size. Those implicit
    /// designs are tracked so asking, or setting a design, undoes them.
    requested_sizes: std::cell::RefCell<std::collections::BTreeMap<WindowId, (f32, f32)>>,
    canvases_that_asked: std::cell::RefCell<std::collections::BTreeSet<u64>>,
    /// Windows whose app asked what screen it is on (krate:ui/screen): an
    /// app that lays out from the screen's numbers is as responsive as one
    /// that asks `canvas_size`, and must not be given a design to shrink.
    screen_windows: std::cell::RefCell<Vec<WindowId>>,
    /// The phone profile's cost measure (IC-915). `KRATE_DRIVE_SCROLL=N`
    /// (headless only) feeds the app N scroll steps through its own wait,
    /// then closes it; each step's cost is the time from handing the app
    /// its event to the app presenting the frame that answers it -- the
    /// app's own work, which a phone runs under the interpreter.
    drive_scroll_left: std::cell::Cell<Option<u32>>,
    drive_scroll_total: u32,
    guest_since: std::cell::Cell<Option<std::time::Instant>>,
    frame_costs: std::cell::RefCell<Vec<f32>>,
    /// When the host was made, and how long the app took from then to
    /// waiting for its first input with a frame drawn (IC-915).
    born: std::time::Instant,
    first_wait_ms: std::cell::Cell<Option<f32>>,
    /// The keys the app has asked about with `key-held`, in the order it
    /// first asked: what an on-screen pad is built from (IC-914).
    keys_asked: std::cell::RefCell<Vec<String>>,
    /// The app reads touch itself (`krate:ui/touch`): it gets no pad.
    reads_touch: std::cell::Cell<bool>,
    /// The pad's keys held by fingers now, in the app's own spellings.
    pad_held: std::cell::RefCell<std::collections::BTreeSet<String>>,
    /// The pad and where it sits, while one is shown.
    pad: std::cell::RefCell<Option<(crate::touch_pad::Pad, crate::touch_pad::Layout)>>,
    /// The fingers on the screen when the pad last looked (for the knob).
    pad_fingers: std::cell::RefCell<Vec<(f32, f32)>>,
    /// When a finger was last on the pad: it rests dimmer when left alone.
    pad_touched_at: std::cell::Cell<std::time::Instant>,
    implicit_designs: std::cell::RefCell<std::collections::BTreeSet<u64>>,
    next_kept_image: std::cell::Cell<u64>,
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
    scenes: std::cell::RefCell<std::collections::BTreeMap<u64, (WindowId, WidgetId, SceneBackend)>>,
    /// The app's GPU objects (krate:gpu): devices, buffers, textures,
    /// pipelines and surfaces, by the handle the app was given.
    gpu: std::cell::RefCell<gpu_host::GpuState>,
    /// The next canvas or scene id to hand out; never reused within a run.
    next_canvas_id: std::cell::Cell<u64>,
    /// Per-canvas display lists, recorded only when the adapter renders
    /// them (`lists_enabled`); the GPU consumer's frame food.
    canvas_lists:
        std::cell::RefCell<std::collections::HashMap<u64, crate::canvas_list::CanvasList>>,
    lists_enabled: std::cell::Cell<Option<bool>>,
    /// Native microphone streams owned by this one sandboxed app session.
    audio_capture: AudioCaptureRuntime,
    /// Open camera streams. Behind `camera.capture`, and idle until an app
    /// opens one -- constructing this touches no device.
    cameras: CameraCaptureRuntime,
    /// Native speaker streams owned by this one sandboxed app session.
    audio_playback: AudioPlaybackRuntime,
    /// Local speech model contexts, scoped to this one sandboxed app session.
    speech: LocalSpeechRuntime,
    /// This app's voice: speaking text aloud, idle until it first speaks.
    speech_out: SpeechSynthesis,
    /// Pages kept for a print job (`krate:ui/print`), until `finish`.
    print_pages: Vec<crate::print_host::Page>,
    /// A screenshot request: paint the window to this PNG at this scale once
    /// the app has drawn a frame. `taken` guards against writing every frame --
    /// the first drawn frame is the one captured.
    screenshot: Option<(std::path::PathBuf, f32)>,
    screenshot_taken: std::cell::Cell<bool>,
    /// `KRATE_SHOOT_AFTER_MS` holds the capture back this long, so the shot is
    /// the app after it has lived a while rather than its first frame. A
    /// camera feed, a fetched image, an animation -- none of them exist in
    /// frame one, so frame one proves nothing about them. The clock starts at
    /// the first capture opportunity; close still captures unconditionally, so
    /// a run shorter than the delay yields its final frame instead of nothing.
    shoot_after: Option<std::time::Duration>,
    shoot_started: std::cell::Cell<Option<std::time::Instant>>,
    /// The stepped monotonic clock, when this run is a screenshot run. The
    /// host ends a frame on it at every present, and the shoot delay counts
    /// its frames instead of real milliseconds. Both halves matter: a stepped
    /// clock nothing advances would freeze the app at time zero, and a delay
    /// still measured in real time would capture a different frame on a fast
    /// machine than a slow one, which is K-721.
    stepped_clock: Option<krate_adapter_common::time::SteppedClock>,
    /// Whether to keep a copy of every draw call for a layout check.
    inspect_layout: std::cell::Cell<bool>,
    /// Draw calls recorded for the current frame, newest frame only.
    inspected_ops: std::cell::RefCell<Vec<crate::canvas_list::CanvasOp>>,
    /// Logical size of the canvas the recorded ops were drawn into.
    inspected_canvas_size: std::cell::Cell<Option<(f32, f32)>>,
    /// Set when a frame was presented, so the next draw op starts a fresh
    /// recording rather than appending to the frame just measured.
    frame_ended: std::cell::Cell<bool>,
    /// The screen as the app's other threads see it (IC-929): the thread that
    /// made this host owns the window (AppKit keeps it in that thread's
    /// state), so a frame another thread presents waits here for it.
    pub(crate) screen: WorkerScreen,
    /// The usability script, when this run is a driven one. `None` on every
    /// ordinary run, which is what keeps this whole mechanism off the path a
    /// person's app takes.
    usability: Option<UsabilityDriver>,
}

/// Wait until `due`. Sleep short, then close the gap by yielding: every OS
/// rounds a sleep UP to its timer granularity, and on Windows that is ~15.6
/// ms, so asking for the exact remainder overshoots by a whole frame.
pub(crate) fn sleep_until(due: std::time::Instant) {
    const SPIN_MARGIN: std::time::Duration = std::time::Duration::from_micros(1_200);
    let remainder = due.saturating_duration_since(std::time::Instant::now());
    if remainder > SPIN_MARGIN {
        std::thread::sleep(remainder - SPIN_MARGIN);
    }
    while std::time::Instant::now() < due {
        std::thread::yield_now();
    }
}

/// What a threaded app's worker threads know of the screen (IC-929).
///
/// A window lives in the state of the thread that made it -- AppKit's on a
/// Mac -- so a worker cannot lower a frame to it or ask its pixel density.
/// Bevy's multi-threaded executor presents from whichever thread runs its
/// render system; the frame is kept here and the main thread lowers it the
/// next time it takes the host, and a worker reads the density the main
/// thread last saw.
pub(crate) struct WorkerScreen {
    main: std::thread::ThreadId,
    /// Windows a worker presented to, not yet lowered, and whether a
    /// `--shoot` picture may be taken of them.
    pub(crate) unsynced: std::cell::RefCell<Vec<(WindowId, bool)>>,
    /// Each window's pixel density, as the main thread last saw it.
    scales: std::cell::RefCell<std::collections::HashMap<WindowId, f32>>,
    /// Whether a worker has touched the screen at all: until one does the
    /// main thread's turn costs nothing.
    pub(crate) used: std::cell::Cell<bool>,
    /// A threaded app: a GPU present's frame pacing waits after the host is
    /// let go (`core_app::paced`), not while holding it.
    // Threaded apps only, which iOS and the web do not run.
    #[cfg_attr(any(target_os = "ios", target_arch = "wasm32"), allow(dead_code))]
    pub(crate) pace_outside: std::cell::Cell<bool>,
}

impl WorkerScreen {
    fn new() -> Self {
        Self {
            main: std::thread::current().id(),
            unsynced: Default::default(),
            scales: Default::default(),
            used: std::cell::Cell::new(false),
            pace_outside: std::cell::Cell::new(false),
        }
    }

    pub(crate) fn on_main(&self) -> bool {
        std::thread::current().id() == self.main
    }
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
    /// When the resize was delivered, so the app gets a turn to redraw at the
    /// new size before it is judged. See `RESIZE_SETTLE`.
    resized_at: Option<std::time::Instant>,
    /// Whether the app has been given a turn to redraw after the resize.
    resize_settled: bool,
    /// When the script last advanced, so a polling game and an event-loop app
    /// are driven at the same pace.
    last_step: Option<std::time::Instant>,
    /// When the press was delivered, so the app gets a turn to react before
    /// the frame is judged. See `PRESS_SETTLE`.
    press_delivered_at: Option<std::time::Instant>,
    /// The task oracle's place: which step is next, and when the last
    /// action was dispatched, so the app gets a settle before the next.
    task_index: usize,
    task_dispatched_at: Option<std::time::Instant>,
    /// How much of a `look` step's motion has been delivered so far.
    look_given: (f32, f32),
    /// The control the pointer step pressed, so the keyboard step can pick a
    /// different one: pressing the same list row twice selects what is
    /// already selected and changes nothing, which read as a keyboard
    /// failure on hello-gui when it was the pointer step's own leftover.
    clicked_widget: Option<WidgetId>,
    /// How much of the frame changes on its own, with nobody touching the app.
    ///
    /// The click check used to pass on ANY difference, which meant a spinner,
    /// a clock, or a game's animation answered for the app: one pixel of
    /// churn read as "it responded" no matter what was pressed, and a
    /// genuinely dead button passed (K-140). Measuring the idle churn first
    /// gives the press something honest to beat.
    idle_churn: Option<f32>,
}

/// How long the app gets to redraw at the new size before the resize is
/// judged, and when to judge anyway.
///
/// Same reasoning as `PRESS_SETTLE`: a canvas refits only when the app draws,
/// and a turn-counted wait can elapse in microseconds (K-154).
const RESIZE_SETTLE: std::time::Duration = std::time::Duration::from_millis(250);
const RESIZE_GIVE_UP: std::time::Duration = std::time::Duration::from_millis(1_500);
/// The last word: judge whatever is on screen. An app that has neither caught
/// up nor come back to `wait` by now genuinely is not following the window.
const RESIZE_HARD_STOP: std::time::Duration = std::time::Duration::from_millis(4_000);

/// How long the app gets to react to the driven press before it is judged.
///
/// The press is handed over from inside `wait`; the guest only handles it,
/// updates its state and redraws after that `wait` returns. Capturing on the
/// next visit landed between those two moments and reported working apps as
/// dead (K-143). A quarter second, once per run, buys a truthful verdict.
const PRESS_SETTLE: std::time::Duration = std::time::Duration::from_millis(250);

/// When to judge the press even though the app never came back to `wait`.
///
/// A frame-loop app (a game) polls and draws forever and never calls `wait`,
/// so waiting for it would stall the script and report the app as having
/// closed itself. After this long the app has had every chance to react and
/// the frame is judged on what is there.
const PRESS_GIVE_UP: std::time::Duration = std::time::Duration::from_millis(1_500);

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
/// One node of the tree as a task step sees it. `kind` is the widget
/// kind's name (`TextField`), matched case-insensitively against a
/// `@text-field` target with the hyphens removed.
struct OnScreen {
    widget: WidgetId,
    kind: String,
    label: String,
    rect: Option<(f32, f32, f32, f32)>,
}

impl OnScreen {
    /// Whether a task target names this node: a label, exactly or by
    /// containment, or `@kind` for the first node of a kind.
    fn matches(&self, target: &str, exact: bool) -> bool {
        let wanted = target.trim().to_lowercase();
        if let Some(kind) = wanted.strip_prefix('@') {
            let kind = kind.replace('-', "");
            return self.kind.to_lowercase() == kind;
        }
        let label = self.label.trim().to_lowercase();
        if label.is_empty() {
            return false;
        }
        if exact {
            label == wanted
        } else {
            label.contains(&wanted)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DriveStep {
    /// Let the app settle and draw its first real frame.
    Settle,
    /// Capture the frame, then resize the window.
    Resize,
    /// Compare the post-resize frame, then deliver a pointer press.
    Click,
    /// Compare the post-click frame, then focus a control and press Enter.
    Keyboard,
    /// Compare the post-keyboard frame, then just watch that it stays open.
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
        libc::signal(
            libc::SIGINT,
            on_interrupt as *const std::ffi::c_void as libc::sighandler_t,
        );
    }
}

fn interrupted() -> bool {
    INTERRUPTED.load(std::sync::atomic::Ordering::SeqCst)
}

/// A window's screen as `screen_facts` answers it: width, height, touch,
/// and the covered edges (top, right, bottom, left), in points.
pub(crate) type ScreenAnswer = (f32, f32, bool, (f32, f32, f32, f32));

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
            headless_pointer: Default::default(),
            idle_waits: std::cell::Cell::new(0),
            last_present: std::cell::Cell::new(None),
            publish_times: std::cell::RefCell::new(Vec::new()),
            present_times: std::cell::RefCell::new(Vec::new()),
            last_present_entry: std::cell::Cell::new(None),
            last_pump: std::cell::Cell::new(None),
            interrupts: std::cell::Cell::new(0),
            app_name: None,
            close_requests: std::cell::Cell::new(0),
            pending_events: std::cell::RefCell::new(std::collections::VecDeque::new()),
            pending_drops: std::cell::RefCell::new(std::collections::VecDeque::new()),
            test_drop_done: std::cell::Cell::new(false),
            headless_started: std::cell::Cell::new(None),
            headless_close_told: std::cell::Cell::new(None),
            images: std::cell::RefCell::new(std::collections::BTreeMap::new()),
            canvases: std::cell::RefCell::new(std::collections::BTreeMap::new()),
            kept_images: std::cell::RefCell::new(std::collections::BTreeMap::new()),
            videos: std::cell::RefCell::new(crate::video::Videos::default()),
            video_frames: std::cell::RefCell::new(std::collections::BTreeMap::new()),
            video_states: std::cell::RefCell::new(std::collections::BTreeMap::new()),
            asset_root: None,
            appearance_cache: std::cell::Cell::new(None),
            described: std::cell::RefCell::new(std::collections::BTreeMap::new()),
            web_pages: std::cell::RefCell::new(std::collections::BTreeMap::new()),
            next_web_page: std::cell::Cell::new(1),
            requested_sizes: std::cell::RefCell::new(std::collections::BTreeMap::new()),
            canvases_that_asked: std::cell::RefCell::new(std::collections::BTreeSet::new()),
            screen_windows: std::cell::RefCell::new(Vec::new()),
            drive_scroll_left: std::cell::Cell::new(
                std::env::var("KRATE_DRIVE_SCROLL")
                    .ok()
                    .and_then(|n| n.trim().parse().ok()),
            ),
            drive_scroll_total: std::env::var("KRATE_DRIVE_SCROLL")
                .ok()
                .and_then(|n| n.trim().parse().ok())
                .unwrap_or(0),
            guest_since: std::cell::Cell::new(None),
            frame_costs: std::cell::RefCell::new(Vec::new()),
            born: std::time::Instant::now(),
            first_wait_ms: std::cell::Cell::new(None),
            // `KRATE_PAD_KEYS` (headless, with `KRATE_SCREEN`): the keys a
            // game would ask about, so the pad can be drawn and checked
            // without playing to the point where it asks.
            keys_asked: std::cell::RefCell::new(
                std::env::var("KRATE_PAD_KEYS")
                    .ok()
                    .filter(|_| std::env::var_os("KRATE_SCREEN").is_some())
                    .map(|keys| {
                        keys.split(',')
                            .map(|k| {
                                if k == "space" {
                                    " ".to_string()
                                } else {
                                    k.to_string()
                                }
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
            ),
            reads_touch: std::cell::Cell::new(false),
            pad_held: std::cell::RefCell::new(std::collections::BTreeSet::new()),
            pad: std::cell::RefCell::new(None),
            pad_fingers: std::cell::RefCell::new(Vec::new()),
            pad_touched_at: std::cell::Cell::new(std::time::Instant::now()),
            implicit_designs: std::cell::RefCell::new(std::collections::BTreeSet::new()),
            next_kept_image: std::cell::Cell::new(1),
            gamepads: std::cell::RefCell::new(crate::gamepad::Gamepads::new()),
            held_keys: std::cell::RefCell::new(std::collections::BTreeSet::new()),
            scenes: std::cell::RefCell::new(std::collections::BTreeMap::new()),
            gpu: Default::default(),
            next_canvas_id: std::cell::Cell::new(1),
            canvas_lists: Default::default(),
            lists_enabled: std::cell::Cell::new(None),
            audio_capture: AudioCaptureRuntime::default(),
            cameras: CameraCaptureRuntime::default(),
            audio_playback: AudioPlaybackRuntime::default(),
            speech: LocalSpeechRuntime::default(),
            speech_out: SpeechSynthesis::default(),
            print_pages: Vec::new(),
            screenshot: None,
            screenshot_taken: std::cell::Cell::new(false),
            shoot_after: std::env::var("KRATE_SHOOT_AFTER_MS")
                .ok()
                .and_then(|value| value.trim().parse::<u64>().ok())
                .map(std::time::Duration::from_millis),
            shoot_started: std::cell::Cell::new(None),
            stepped_clock: None,
            inspect_layout: std::cell::Cell::new(false),
            inspected_ops: std::cell::RefCell::new(Vec::new()),
            inspected_canvas_size: std::cell::Cell::new(None),
            frame_ended: std::cell::Cell::new(false),
            screen: WorkerScreen::new(),
            usability: None,
        })
    }

    /// Serve this run's monotonic time from a stepped clock instead of the
    /// host. Only the screenshot path sets this. See the field's comment.
    pub fn with_stepped_clock(
        mut self,
        clock: Option<krate_adapter_common::time::SteppedClock>,
    ) -> Self {
        self.stepped_clock = clock;
        self
    }

    /// The name the person consented to, for anything shown outside the
    /// app's window (IC-271).
    pub fn with_app_name(mut self, name: Option<String>) -> Self {
        self.app_name = name;
        self
    }

    /// What the OS shows as the sender of a notification.
    ///
    /// Never the notification's own title. The first version passed the
    /// guest's title here, under a comment that read "the app's own title is
    /// used for attribution" -- two meanings of "title" in one sentence, and
    /// the wrong one won. The WIT promises an app "cannot post a notification
    /// that appears to come from somewhere else"; with the guest choosing the
    /// sender line per call, a to-do app could post as "1Password".
    fn attribution(&self) -> String {
        crate::desktop_host::attribution_for(self.app_name.as_deref())
    }

    pub fn with_asset_root(mut self, root: Option<std::path::PathBuf>) -> Self {
        self.asset_root = root.clone();
        self.speech = LocalSpeechRuntime::default().with_asset_root(root);
        self
    }

    /// Drive this run against a usability script instead of letting it idle out.
    ///
    /// Only ever set by `check-app`'s usability stage. An ordinary run passes
    /// `None` and behaves exactly as it always has, which is the point: this is
    /// a harness bolted onto the side of the real run, not a change to it.
    /// Share the chosen-files registry with the fs host (see
    /// Phase2Host::with_chosen_files -- K-083).
    pub fn with_chosen_files(
        mut self,
        chosen: std::rc::Rc<std::cell::RefCell<crate::chosen_files::ChosenFiles>>,
    ) -> Self {
        self.chosen_files = chosen;
        self
    }

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
            resized_at: None,
            resize_settled: false,
            last_step: None,
            press_delivered_at: None,
            task_index: 0,
            task_dispatched_at: None,
            look_given: (0.0, 0.0),
            clicked_widget: None,
            idle_churn: None,
        });
        self
    }

    /// Ask the host to paint the window to `path` at `scale` once the app has
    /// drawn a frame. Only meaningful on a headless run, where there is no real
    /// window to grab from the window server.
    /// Record every draw call so the captured frame can be checked for text
    /// drawn over text.
    pub fn with_layout_check(self, enabled: bool) -> Self {
        self.inspect_layout.set(enabled);
        self
    }

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
    /// The main thread's turn at the host, in a threaded app (IC-929): lower
    /// the frames workers presented since, and note each window's density
    /// for them. Free until a worker first touches the screen.
    // Threaded apps only, which iOS and the web do not run.
    #[cfg_attr(any(target_os = "ios", target_arch = "wasm32"), allow(dead_code))]
    pub(crate) fn main_turn(&self) {
        if !self.screen.used.get() || !self.screen.on_main() {
            return;
        }
        let presented = std::mem::take(&mut *self.screen.unsynced.borrow_mut());
        for (window, worth_shooting) in presented {
            if let Err(error) = self.sync_native_widgets(window) {
                tracing::debug!(?error, "a frame a worker presented could not be lowered");
            } else if worth_shooting {
                self.maybe_take_screenshot();
            }
        }
        for window in self.windows.iter().copied() {
            let scale = self.dispatcher().window_scale(window);
            self.screen.scales.borrow_mut().insert(window, scale);
        }
    }

    /// A worker presented to `window`: its own thread lowers the frame on
    /// its next turn (WorkerScreen).
    pub(crate) fn hand_frame_to_main(&self, window: WindowId, worth_shooting: bool) {
        self.screen.used.set(true);
        let mut unsynced = self.screen.unsynced.borrow_mut();
        match unsynced.iter_mut().find(|(w, _)| *w == window) {
            Some(entry) => entry.1 |= worth_shooting,
            None => unsynced.push((window, worth_shooting)),
        }
    }

    fn maybe_take_screenshot(&self) {
        // A run that ends before it ever waits still shows the person's look.
        self.follow_widget_look();
        let Some(window) = self.windows.first().copied() else {
            return;
        };
        if !self.shoot_delay_elapsed() {
            return;
        }
        self.maybe_take_screenshot_for(window);
    }

    /// Move a stepped clock on by one frame. Called after a present, so every
    /// clock read the app made while drawing that frame saw one instant.
    ///
    /// A no-op on an ordinary run, where there is no stepped clock and the app
    /// reads real time.
    fn end_stepped_frame(&self) {
        if let Some(stepped) = self.stepped_clock.as_ref() {
            stepped.end_frame();
        }
    }

    /// Whether `KRATE_SHOOT_AFTER_MS` has passed. Without the knob this is
    /// always true and the first frame is the one captured, as before. The
    /// clock starts at the first capture opportunity, not host construction,
    /// so instantiation time does not eat into the delay.
    fn shoot_delay_elapsed(&self) -> bool {
        let Some(delay) = self.shoot_after else {
            return true;
        };
        // On a stepped clock the delay is counted in the app's own frames, not
        // in real milliseconds. Real milliseconds are what made this random:
        // 400 ms is more frames on a fast machine than a slow one, and an
        // animated app is in a different pose in each. Frames are not, so the
        // same run always captures the same one.
        if let Some(stepped) = self.stepped_clock.as_ref() {
            let step = stepped.step_nanos();
            if step > 0 {
                let wanted = (delay.as_nanos() as u64).div_ceil(step);
                return stepped.frames() >= wanted;
            }
        }
        let now = std::time::Instant::now();
        match self.shoot_started.get() {
            None => {
                self.shoot_started.set(Some(now));
                delay.is_zero()
            }
            Some(started) => now.duration_since(started) >= delay,
        }
    }

    /// Capture a specific window, used on close so an app that never waits for
    /// input still gets its final frame shot.
    fn maybe_take_screenshot_for(&self, window: WindowId) {
        self.follow_widget_look();
        if self.screenshot_taken.get() || self.screenshot.is_none() {
            return;
        }
        let Some((path, scale)) = self.screenshot.as_ref() else {
            return;
        };
        match self.render_window_png(window, *scale, path) {
            Ok(()) => {
                self.screenshot_taken.set(true);
                self.report_text_overlaps();
            }
            // Nothing to render yet is not an error: the very first wait can
            // arrive before the tree exists. A later wait will succeed.
            Err(error) => tracing::debug!(?error, "screenshot not ready yet"),
        }
    }

    fn dispatcher(&self) -> Phase3UiDispatcher<'_> {
        self.runtime.dispatcher()
    }

    /// Schedule a reminder (IC-921), behind `ui.notify`: the id checked
    /// (it becomes part of the system's identifier), the text checked as a
    /// notification's is, the delay held to a second to a year.
    pub(crate) fn reminder_schedule(
        &self,
        id: &str,
        title: &str,
        body: &str,
        after_seconds: u64,
        daily: bool,
    ) -> Result<(), crate::desktop_host::NotifyError> {
        use crate::desktop_host::NotifyError;
        if self
            .runtime
            .guard()
            .check(&UapiCall::Ui(UiCall::Notify))
            .is_err()
        {
            return Err(NotifyError::Denied);
        }
        if !valid_reminder_id(id) {
            return Err(NotifyError::InvalidContent(
                "a reminder id is 1 to 64 letters, digits, - and _".to_string(),
            ));
        }
        crate::desktop_host::check_notification(title, body)?;
        if !(1..=366 * 24 * 3600).contains(&after_seconds) {
            return Err(NotifyError::InvalidContent(
                "a reminder comes 1 second to a year from now".to_string(),
            ));
        }
        if self.headless {
            return Err(NotifyError::Unavailable(
                "no reminders when nothing is on screen".to_string(),
            ));
        }
        let sender = self.attribution();
        self.dispatcher()
            .reminders()
            .reminder_schedule(id, &sender, title, body, after_seconds, daily)
            .map_err(NotifyError::Unavailable)
    }

    pub(crate) fn reminder_cancel(&self, id: &str) {
        let granted = self
            .runtime
            .guard()
            .check(&UapiCall::Ui(UiCall::Notify))
            .is_ok();
        if granted && valid_reminder_id(id) && !self.headless {
            self.dispatcher().reminders().reminder_cancel(id);
        }
    }

    pub(crate) fn reminder_pending(&self) -> Vec<String> {
        let granted = self
            .runtime
            .guard()
            .check(&UapiCall::Ui(UiCall::Notify))
            .is_ok();
        if !granted || self.headless {
            return Vec::new();
        }
        self.dispatcher().reminders().reminder_pending()
    }

    /// Pick photos (IC-920): the phone's photo picker, or on a computer the
    /// file dialog for pictures; each pick remembered as a chosen file.
    /// Headless answers as cancelled, as open-file does.
    pub(crate) fn pick_photos(
        &self,
        window: u64,
        limit: u32,
    ) -> Result<Vec<(String, String)>, (u8, String)> {
        let limit = limit.clamp(1, 20);
        if self.headless {
            return Ok(Vec::new());
        }
        let id = self
            .window_id(window)
            .map_err(|_| (2, "no such window".to_string()))?;
        let picked: Vec<std::path::PathBuf> = match self.dispatcher().pick_photos(id, limit) {
            Some(Ok(paths)) => paths,
            Some(Err(why)) => return Err((1, why)),
            None => {
                #[cfg(not(any(target_os = "android", target_os = "ios", target_arch = "wasm32")))]
                {
                    let mut dialog = rfd::FileDialog::new()
                        .set_title("Choose photos")
                        .add_filter("Pictures", &["jpg", "jpeg", "png", "heic", "gif", "webp"]);
                    let _ = &mut dialog;
                    let files = if limit == 1 {
                        dialog.pick_file().into_iter().collect::<Vec<_>>()
                    } else {
                        dialog.pick_files().unwrap_or_default()
                    };
                    files.into_iter().take(limit as usize).collect()
                }
                #[cfg(any(target_os = "android", target_os = "ios", target_arch = "wasm32"))]
                {
                    return Err((1, "no photo picker on this device yet".to_string()));
                }
            }
        };
        let mut chosen = Vec::new();
        for path in picked {
            let name = crate::chosen_files::ChosenFiles::display_name(&path);
            let Some(token) = self.chosen_files.borrow_mut().remember(path) else {
                return Err((1, "too many files chosen in one run".to_string()));
            };
            chosen.push((name, token));
        }
        Ok(chosen)
    }

    /// Add a picture to the photo library (IC-920), behind `photos.add`;
    /// only PNG and JPEG, at most 64 MB.
    pub(crate) fn save_photo(&self, data: &[u8]) -> Result<(), (u8, String)> {
        if self
            .runtime
            .guard()
            .check(&UapiCall::Photos(crate::uapi::PhotosCall::Add))
            .is_err()
        {
            return Err((0, String::new()));
        }
        let png = data.starts_with(&[0x89, b'P', b'N', b'G']);
        let jpeg = data.starts_with(&[0xFF, 0xD8, 0xFF]);
        if !(png || jpeg) {
            return Err((2, "only PNG and JPEG pictures can be added".to_string()));
        }
        if data.len() > 64 * 1024 * 1024 {
            return Err((2, "the picture is over 64 MB".to_string()));
        }
        if self.headless {
            return Err((1, "no photo library when nothing is on screen".to_string()));
        }
        self.dispatcher().save_photo(data).map_err(|why| match why {
            None => (0, String::new()),
            Some(why) => (1, why),
        })
    }

    /// Open the share sheet (IC-919), after checking what is offered: at
    /// least one thing, file names without a path, 64 MB in all.
    pub(crate) fn share(
        &self,
        window: u64,
        text: Option<String>,
        url: Option<String>,
        files: Vec<(String, Vec<u8>)>,
    ) -> Result<bool, (bool, String)> {
        const MAX_BYTES: usize = 64 * 1024 * 1024;
        let empty = |s: &Option<String>| s.as_deref().is_none_or(|t| t.trim().is_empty());
        if empty(&text) && empty(&url) && files.is_empty() {
            return Err((false, "nothing to share".to_string()));
        }
        for (name, _) in &files {
            let bare = std::path::Path::new(name)
                .file_name()
                .is_some_and(|leaf| leaf == std::ffi::OsStr::new(name));
            if name.trim().is_empty() || !bare || name.starts_with('.') {
                return Err((
                    false,
                    format!("a file name must be a plain name, not {name:?}"),
                ));
            }
        }
        let total: usize = files.iter().map(|(_, data)| data.len()).sum::<usize>()
            + text.as_ref().map_or(0, String::len)
            + url.as_ref().map_or(0, String::len);
        if total > MAX_BYTES {
            return Err((false, "more than 64 MB to share".to_string()));
        }
        if self.headless {
            return Err((true, "no share sheet when nothing is on screen".to_string()));
        }
        let id = self
            .window_id(window)
            .map_err(|_| (false, "no such window".to_string()))?;
        self.dispatcher()
            .share(id, text.as_deref(), url.as_deref(), &files)
            .map_err(|why| (true, why))
    }

    /// How the person asked things to look (IC-925), read at most twice a
    /// second: an app asks every frame and the platform answers on its main
    /// thread. Headless, `KRATE_APPEARANCE` stands in (`dark,text=1.35`).
    pub(crate) fn appearance(&self) -> krate_adapter_common::ui::Appearance {
        if self.headless {
            return std::env::var("KRATE_APPEARANCE")
                .map(|text| krate_adapter_common::ui::Appearance::parse(&text))
                .unwrap_or_default();
        }
        let now = std::time::Instant::now();
        if let Some((at, look)) = self.appearance_cache.get() {
            if now.duration_since(at) < std::time::Duration::from_millis(500) {
                return look;
            }
        }
        let look = self.dispatcher().appearance();
        self.appearance_cache.set(Some((now, look)));
        look
    }

    /// The hosts a web page may go to (IC-927): each `net.connect` (or
    /// `net.fetch`) grant's host and port.
    fn web_page_hosts(&self) -> Vec<(String, String)> {
        self.runtime
            .guard()
            .policy()
            .grants()
            .iter()
            .map(ToString::to_string)
            .filter_map(|cap| {
                let rest = cap
                    .strip_prefix("net.connect:")
                    .or_else(|| cap.strip_prefix("net.fetch:"))?;
                let (host, port) = rest.rsplit_once(':').unwrap_or((rest, "443"));
                Some((host.to_string(), port.to_string()))
            })
            .collect()
    }

    /// Show a web page in a box of a window (IC-927), when its address is
    /// one the app may open.
    pub(crate) fn open_web_page(
        &self,
        window: u64,
        address: &str,
        frame: (f32, f32, f32, f32),
    ) -> Result<u64, ui::types::UiError> {
        let id = self.window_id(window)?;
        let hosts = self.web_page_hosts();
        krate_adapter_common::net::web_page_allowed(address, &hosts)
            .map_err(|_| ui::types::UiError::PermissionDenied)?;
        let page = if self.headless {
            let page = self.next_web_page.get();
            self.next_web_page.set(page + 1);
            page
        } else {
            self.dispatcher()
                .bluetooth()
                .open_web_page(id, address, frame, hosts)
                .map_err(ui::types::UiError::Unsupported)?
        };
        self.web_pages.borrow_mut().insert(
            page,
            (
                window,
                krate_adapter_common::ui::WebPageState {
                    address: address.to_string(),
                    ..Default::default()
                },
            ),
        );
        Ok(page)
    }

    /// Whether this run opened `page`.
    pub(crate) fn owns_web_page(&self, page: u64) -> bool {
        self.web_pages.borrow().contains_key(&page)
    }

    /// Send a page this run owns to another address, judged as at open.
    pub(crate) fn web_page_go(&self, page: u64, address: &str) -> Result<(), ui::types::UiError> {
        if !self.owns_web_page(page) {
            return Err(ui::types::UiError::InvalidWidget);
        }
        if let Err(why) =
            krate_adapter_common::net::web_page_allowed(address, &self.web_page_hosts())
        {
            if let Some((_, state)) = self.web_pages.borrow_mut().get_mut(&page) {
                state.problem = why;
            }
            return Err(ui::types::UiError::PermissionDenied);
        }
        if self.headless {
            if let Some((_, state)) = self.web_pages.borrow_mut().get_mut(&page) {
                state.address = address.to_string();
                state.problem.clear();
            }
        } else {
            self.dispatcher().bluetooth().web_page_go(page, address);
        }
        Ok(())
    }

    /// Where a page this run owns is, and what it is doing.
    pub(crate) fn web_page_state(
        &self,
        page: u64,
    ) -> Result<krate_adapter_common::ui::WebPageState, ui::types::UiError> {
        let held = self
            .web_pages
            .borrow()
            .get(&page)
            .map(|(_, state)| state.clone())
            .ok_or(ui::types::UiError::InvalidWidget)?;
        if self.headless {
            return Ok(held);
        }
        let mut live = self
            .dispatcher()
            .bluetooth()
            .web_page_state(page)
            .unwrap_or(held.clone());
        // A refusal the host made (a `go` to an ungranted host) is said too.
        if live.problem.is_empty() {
            live.problem = held.problem;
        }
        Ok(live)
    }

    /// Whether nothing is on screen (a headless run).
    pub(crate) fn headless_run(&self) -> bool {
        self.headless
    }

    /// Move a page's box, or send it back a page.
    pub(crate) fn place_web_page(&self, page: u64, frame: (f32, f32, f32, f32)) {
        self.dispatcher().bluetooth().place_web_page(page, frame);
    }

    pub(crate) fn web_page_back(&self, page: u64) {
        self.dispatcher().bluetooth().web_page_back(page);
    }

    /// Take a page this run owns away.
    pub(crate) fn close_web_page(&self, page: u64) {
        if self.web_pages.borrow_mut().remove(&page).is_some() && !self.headless {
            self.dispatcher().bluetooth().close_web_page(page);
        }
    }

    /// The most elements one description may carry, and the longest label.
    const MAX_ACCESS_ELEMENTS: usize = 2000;
    const MAX_ACCESS_TEXT: usize = 500;

    /// Tell the screen reader what a window shows (IC-926). Headless, the
    /// description is kept, and written as JSON to `KRATE_ACCESS_DUMP`.
    pub(crate) fn describe_accessibility(
        &self,
        window: u64,
        mut elements: Vec<krate_adapter_common::ui::AccessElement>,
    ) -> Result<(), ui::types::UiError> {
        let id = self.window_id(window)?;
        elements.truncate(Self::MAX_ACCESS_ELEMENTS);
        for element in &mut elements {
            for text in [&mut element.label, &mut element.value] {
                if text.len() > Self::MAX_ACCESS_TEXT {
                    let mut cut = Self::MAX_ACCESS_TEXT;
                    while !text.is_char_boundary(cut) {
                        cut -= 1;
                    }
                    text.truncate(cut);
                }
            }
        }
        if let Some(path) = std::env::var_os("KRATE_ACCESS_DUMP") {
            let dump: Vec<serde_json::Value> = elements
                .iter()
                .map(|e| {
                    serde_json::json!({
                        "role": format!("{:?}", e.role).to_lowercase(),
                        "label": e.label,
                        "value": e.value,
                        "checked": e.checked,
                        "enabled": e.enabled,
                        "frame": [e.x, e.y, e.width, e.height],
                    })
                })
                .collect();
            let _ = std::fs::write(path, serde_json::Value::Array(dump).to_string());
        }
        let result = if self.headless {
            Ok(())
        } else {
            self.dispatcher()
                .describe_accessibility(id, &elements)
                .map_err(|error| ui::types::UiError::Platform(error.to_string()))
        };
        // check-app's frame report is written at `present`, and an app
        // describes the frame just after drawing it: mark the report too.
        if !elements.is_empty() {
            if let Some(path) = std::env::var_os("KRATE_FIT_REPORT") {
                if let Some(mut report) = std::fs::read_to_string(&path)
                    .ok()
                    .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
                {
                    report["described"] = serde_json::Value::Bool(true);
                    let _ = std::fs::write(&path, report.to_string());
                }
            }
        }
        self.described.borrow_mut().insert(window, elements);
        result
    }

    /// Whether a screen reader is on (IC-926). Headless,
    /// `KRATE_SCREEN_READER=1` stands in for one.
    pub(crate) fn screen_reader_on(&self) -> bool {
        if self.headless {
            return std::env::var("KRATE_SCREEN_READER").as_deref() == Ok("1");
        }
        self.dispatcher().screen_reader_on()
    }

    /// Draw Krate's own widgets in the person's look (IC-925), checked at
    /// the top of every frame: dark colours in dark mode, labels at their
    /// text size, laid out at that size on the next layout.
    fn follow_widget_look(&self) {
        let look = self.appearance();
        if look != krate_adapter_common::ui::widget_look() {
            krate_adapter_common::ui::set_widget_look(look);
        }
    }

    /// Whether this device can check it is the person (IC-924). Headless,
    /// only when `KRATE_UNLOCK` stands in for one that can.
    pub(crate) fn unlock_available(&self) -> bool {
        if self.headless {
            return matches!(std::env::var("KRATE_UNLOCK").as_deref(), Ok("yes" | "no"));
        }
        crate::unlock::available()
    }

    /// Ask the person to prove it is them (IC-924). Headless, `KRATE_UNLOCK`
    /// stands in for them: `yes`, `no`, or anything else for a device that
    /// cannot check.
    pub(crate) fn unlock_check(&self, reason: &str) -> crate::unlock::Answer {
        if let Err(why) = crate::unlock::check_reason(reason) {
            return crate::unlock::Answer::Unavailable(why);
        }
        if self.headless {
            return match std::env::var("KRATE_UNLOCK") {
                Ok(answer) => crate::unlock::stand_in(&answer),
                Err(_) => crate::unlock::Answer::Unavailable(
                    "nothing is on screen to ask with".to_string(),
                ),
            };
        }
        crate::unlock::check(reason.trim())
    }

    /// Sign the person in through the system's sheet (IC-923): the address
    /// the service sent them back to, None when they closed it; Err((true,
    /// why)) when there is no sheet, Err((false, why)) for a bad request.
    /// Headless, `KRATE_SIGN_IN` stands in for the person: `cancel`, or the
    /// code the service sends back.
    pub(crate) fn sign_in(
        &self,
        window: u64,
        authorize_url: &str,
        scheme: &str,
    ) -> Result<Option<String>, (bool, String)> {
        crate::sign_in::check(authorize_url, scheme).map_err(|why| (false, why))?;
        if self.headless {
            return match std::env::var("KRATE_SIGN_IN") {
                Ok(answer) => crate::sign_in::stand_in(&answer, authorize_url, scheme)
                    .map_err(|why| (true, why)),
                Err(_) => Err((
                    true,
                    "no sign-in sheet when nothing is on screen".to_string(),
                )),
            };
        }
        let id = self
            .window_id(window)
            .map_err(|_| (false, "no such window".to_string()))?;
        self.dispatcher()
            .sign_in(id, authorize_url, scheme)
            .map_err(|why| (true, why))
    }

    /// Whether `bluetooth.connect` was granted (IC-917).
    fn bluetooth_granted(&self) -> bool {
        self.runtime
            .guard()
            .check(&UapiCall::Bluetooth(crate::uapi::BluetoothCall::Connect))
            .is_ok()
    }

    /// Whether this device has Bluetooth (IC-917). Headless with
    /// `KRATE_BLUETOOTH=heart`, a stand-in heart-rate strap is nearby.
    pub(crate) fn bluetooth_available(&self) -> bool {
        if self.headless {
            return fake_heart();
        }
        self.dispatcher().bluetooth().bluetooth_available()
    }

    /// Krate's chooser, then the picked device (IC-917). Not granted, the
    /// person is never shown a list and nothing is scanned.
    pub(crate) fn bluetooth_choose(
        &self,
        window: u64,
        service: &str,
        title: &str,
    ) -> Result<Option<krate_adapter_common::ui::BtDevice>, krate_adapter_common::ui::BtError> {
        use krate_adapter_common::ui::{BtDevice, BtError};
        if !self.bluetooth_granted() {
            return Err(BtError::NotAllowed);
        }
        if self.headless {
            return match fake_heart() {
                true if short_uuid(service) == "180D" => Ok(Some(BtDevice {
                    handle: 1,
                    name: "Test Heart".to_string(),
                })),
                true => Err(BtError::NotFound),
                false => Err(BtError::Unavailable),
            };
        }
        let id = self.window_id(window).map_err(|_| BtError::NotFound)?;
        self.dispatcher()
            .bluetooth()
            .bluetooth_choose(id, service, title)
    }

    /// Read, write, subscribe, collect, check and let go of a picked
    /// device (IC-917): each behind the grant, the stand-in headless.
    pub(crate) fn bluetooth_read(
        &self,
        device: u64,
        service: &str,
        characteristic: &str,
    ) -> Result<Vec<u8>, krate_adapter_common::ui::BtError> {
        use krate_adapter_common::ui::BtError;
        if !self.bluetooth_granted() {
            return Err(BtError::NotAllowed);
        }
        if self.headless {
            return match (
                device,
                short_uuid(service).as_str(),
                short_uuid(characteristic).as_str(),
            ) {
                (1, "180F", "2A19") => Ok(vec![87]),
                (1, _, _) => Err(BtError::NotFound),
                _ => Err(BtError::Disconnected),
            };
        }
        self.dispatcher()
            .bluetooth()
            .bluetooth_read(device, service, characteristic)
    }

    pub(crate) fn bluetooth_write(
        &self,
        device: u64,
        service: &str,
        characteristic: &str,
        data: &[u8],
        with_response: bool,
    ) -> Result<(), krate_adapter_common::ui::BtError> {
        use krate_adapter_common::ui::BtError;
        if !self.bluetooth_granted() {
            return Err(BtError::NotAllowed);
        }
        if self.headless {
            return if device == 1 {
                Ok(())
            } else {
                Err(BtError::Disconnected)
            };
        }
        self.dispatcher().bluetooth().bluetooth_write(
            device,
            service,
            characteristic,
            data,
            with_response,
        )
    }

    pub(crate) fn bluetooth_subscribe(
        &self,
        device: u64,
        service: &str,
        characteristic: &str,
    ) -> Result<(), krate_adapter_common::ui::BtError> {
        use krate_adapter_common::ui::BtError;
        if !self.bluetooth_granted() {
            return Err(BtError::NotAllowed);
        }
        if self.headless {
            return match (
                device,
                short_uuid(service).as_str(),
                short_uuid(characteristic).as_str(),
            ) {
                (1, "180D", "2A37") => Ok(()),
                (1, _, _) => Err(BtError::NotFound),
                _ => Err(BtError::Disconnected),
            };
        }
        self.dispatcher()
            .bluetooth()
            .bluetooth_subscribe(device, service, characteristic)
    }

    pub(crate) fn bluetooth_notifications(
        &self,
        device: u64,
    ) -> Vec<krate_adapter_common::ui::BtNotification> {
        if !self.bluetooth_granted() {
            return Vec::new();
        }
        if self.headless {
            // One heartbeat each ask: flags 0, then 72 beats a minute.
            return if device == 1 {
                vec![("180D".to_string(), "2A37".to_string(), vec![0, 72])]
            } else {
                Vec::new()
            };
        }
        self.dispatcher()
            .bluetooth()
            .bluetooth_notifications(device)
    }

    pub(crate) fn bluetooth_connected(&self, device: u64) -> bool {
        if !self.bluetooth_granted() {
            return false;
        }
        if self.headless {
            return device == 1 && fake_heart();
        }
        self.dispatcher().bluetooth().bluetooth_connected(device)
    }

    pub(crate) fn bluetooth_disconnect(&self, device: u64) {
        if self.bluetooth_granted() && !self.headless {
            self.dispatcher().bluetooth().bluetooth_disconnect(device);
        }
    }

    /// Where the device is (IC-917), behind `location.read`: an app that
    /// was not granted it hears `not-allowed` and the phone is never asked.
    /// Headless, `KRATE_LOCATION=lat,lon,accuracy` stands in for a fix.
    pub(crate) fn location(&self) -> (bool, krate_adapter_common::ui::LocationAnswer) {
        use krate_adapter_common::ui::{LocationAnswer, Position};
        let granted = self
            .runtime
            .guard()
            .check(&UapiCall::Location(crate::uapi::LocationCall::Read))
            .is_ok();
        if self.headless {
            let fix = std::env::var("KRATE_LOCATION").ok().and_then(|spec| {
                let v: Vec<f64> = spec
                    .split(',')
                    .filter_map(|n| n.trim().parse().ok())
                    .collect();
                (v.len() == 3).then(|| Position {
                    latitude: v[0],
                    longitude: v[1],
                    accuracy: v[2] as f32,
                    ..Default::default()
                })
            });
            let answer = match fix {
                _ if !granted => LocationAnswer::NotAllowed,
                Some(fix) => LocationAnswer::Here(fix),
                None => LocationAnswer::Unavailable,
            };
            return (fix.is_some(), answer);
        }
        let dispatcher = self.dispatcher();
        let available = dispatcher.location_available();
        if !granted {
            return (available, LocationAnswer::NotAllowed);
        }
        (available, dispatcher.location())
    }

    /// How the device is held (IC-917). Headless, `KRATE_MOTION=gx,gy,gz`
    /// stands in for a phone held so, to check an app's tilt handling.
    pub(crate) fn motion(&self) -> (bool, Option<krate_adapter_common::ui::MotionReading>) {
        if self.headless {
            let held = std::env::var("KRATE_MOTION").ok().and_then(|spec| {
                let v: Vec<f32> = spec
                    .split(',')
                    .filter_map(|n| n.trim().parse().ok())
                    .collect();
                (v.len() == 3).then(|| krate_adapter_common::ui::MotionReading {
                    gravity: (v[0], v[1], v[2]),
                    heading: Some(0.0),
                    ..Default::default()
                })
            });
            return (held.is_some(), held);
        }
        let dispatcher = self.dispatcher();
        (dispatcher.motion_available(), dispatcher.motion())
    }

    /// Whether the device can tap back (IC-917); never headless.
    pub(crate) fn haptics_available(&self) -> bool {
        !self.headless && self.dispatcher().haptics_available()
    }

    /// Tap back (IC-917): a feel, or an impact at `strength` when `feel` is
    /// None. Headless runs have no one holding anything.
    pub(crate) fn haptic(&self, feel: Option<krate_adapter_common::ui::HapticFeel>, strength: f32) {
        if !self.headless {
            self.dispatcher().haptic(feel, strength);
        }
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
    /// Pump the native event loop from outside the guest's own event calls.
    ///
    /// The epoch callback uses this: it fires *inside* running guest code,
    /// where none of the ordinary event paths are reachable. See K-032.
    pub fn pump_native_windows_for_epoch(&self) {
        self.pump_native_windows();
    }

    fn pump_native_windows(&self) {
        if self.headless {
            return;
        }
        // Coalesce: an app that polls in a loop asked the OS for events four
        // times a frame, and each ask costs a round trip through the window
        // server -- 1.5ms apiece, 6ms of a 16ms frame, the largest single
        // cost on the CPU present path. Events arriving 0.4ms apart cannot
        // matter to a 60Hz frame, so one pump per 4ms is indistinguishable
        // to a person and four times cheaper. `present` clears the mark, so
        // every frame still begins with a fresh look at the queue.
        const COALESCE: std::time::Duration = std::time::Duration::from_millis(8);
        if let Some(last) = self.last_pump.get() {
            if last.elapsed() < COALESCE {
                return;
            }
        }
        self.last_pump.set(Some(std::time::Instant::now()));
        let __pump_guard = PumpTimer(std::time::Instant::now());
        let _ = &__pump_guard;
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
    fn drive_usability_step(&mut self, from_wait: bool) -> Option<ui::types::Event> {
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
                    // Spend one extra visit measuring how much the app changes
                    // on its own. A spinner or an animation makes every frame
                    // differ from the last, and without this number the click
                    // check credits that churn to the press (K-140).
                    if driver.idle_churn.is_none() {
                        match (driver.before.as_ref(), frame.as_ref()) {
                            (Some(previous), Some(current)) => {
                                driver.idle_churn =
                                    Some(crate::usability::frame_difference(previous, current));
                            }
                            _ => {
                                // First content frame: hold it and come back a
                                // step later to see what moved by itself.
                                driver.before = frame;
                                return None;
                            }
                        }
                    }
                    driver.before = frame;
                    driver.step = if driver.plan.check_resize {
                        DriveStep::Resize
                    } else if driver.plan.check_click {
                        DriveStep::Click
                    } else if driver.plan.check_keyboard {
                        DriveStep::Keyboard
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
                    if driver.plan.check_keyboard {
                        driver.report.keyboard =
                            Some(crate::usability::Observation::unobserved(reason));
                    }
                    driver.step = DriveStep::Watch;
                }
                None
            }
            DriveStep::Resize => {
                self.drive_resize(window, from_wait);
                None
            }
            DriveStep::Click => {
                self.drive_click(window, from_wait);
                None
            }
            DriveStep::Keyboard => {
                self.drive_keyboard(window, from_wait);
                None
            }
            DriveStep::Watch => {
                // The primary task first, once, when the plan has one
                // (IC-743, test 1506): the other checks proved the app
                // answers; this proves it does its job.
                let task_pending = self
                    .usability
                    .as_ref()
                    .is_some_and(|d| d.plan.task.is_some() && d.report.task.is_none());
                if task_pending {
                    self.drive_task(window, from_wait);
                    return None;
                }
                let driver = self.usability.as_mut()?;
                let started = driver.started?;
                // Nothing to watch for: a run that only does its task (`krate
                // run --task`) ends when the task does, not fifteen seconds on.
                if !driver.plan.check_stay_open {
                    driver.step = DriveStep::Done;
                    return Some(ui::types::Event::CloseRequested(window.get()));
                }
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
    fn drive_resize(&mut self, window: WindowId, from_wait: bool) {
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
                driver.resized_at = Some(std::time::Instant::now());
            }
            return;
        }

        // Give the app a real turn before judging it.
        //
        // A canvas is only refitted when the app DRAWS, so judging before the
        // next frame fails apps that were about to do exactly the right thing.
        // This used to be one extra visit -- a turn counter, not time -- and a
        // visit can land in microseconds. Measured: the same app failed on one
        // run in three, reporting "the window grew but the app kept drawing at
        // its old size" for an app that resizes correctly (K-154).
        //
        // Wall clock, with the same shape the press check uses (K-143): the
        // app has had its slice AND the judging visit comes from `wait`, where
        // this turn is finished. `RESIZE_GIVE_UP` judges anyway for a frame-loop
        // app that never calls `wait`.
        if let Some(driver) = self.usability.as_mut() {
            if !driver.resize_settled {
                driver.resize_settled = true;
                return;
            }
        }
        let since_resize = self
            .usability
            .as_ref()
            .and_then(|d| d.resized_at)
            .map(|at| at.elapsed())
            .unwrap_or_default();
        // The give-up path is a floor, not a shortcut: it only judges once the
        // canvas has ALREADY caught up, or once so long has passed that the app
        // is clearly not going to redraw. Judging on the clock alone re-created
        // the very race this is meant to close -- an app whose event loop waits
        // 220ms between rounds can still be one frame behind at 1.5s, and it
        // was reported as ignoring the window on one run in five.
        // "Caught up" has to mean the thing the VERDICT judges, which is the
        // surface the app is drawing into -- not the layout rect.
        //
        // Two false starts here, both worth recording. Comparing the canvas
        // rect to the window never matches, because a canvas is inset. Then
        // comparing the rect to its old size passes as soon as the LAYOUT
        // reflows -- which happens before the app has redrawn, so the verdict
        // still read the old render size and failed a correct app on about one
        // run in eight. The layout follows the window on its own; only the
        // render size proves the app itself caught up (K-154).
        let caught_up = match (
            self.canvas_rect(window),
            self.canvas_pixels_for_window(window),
        ) {
            (Some((cw, ch)), Some((rw, rh))) => {
                (rw as f32 - cw).abs() <= 2.0 && (rh as f32 - ch).abs() <= 2.0
            }
            // No canvas2d surface at all (a 3D scene or a widget app): there is
            // nothing to wait for, and the verdict exempts these anyway.
            (Some(_), None) => true,
            _ => false,
        };
        if std::env::var_os("KRATE_EVENT_TRACE").is_some() {
            eprintln!(
                "krate-resize: since={}ms from_wait={from_wait} caught_up={caught_up}",
                since_resize.as_millis(),
            );
        }
        let ready = (since_resize >= RESIZE_SETTLE && from_wait && caught_up)
            || (since_resize >= RESIZE_GIVE_UP && (caught_up || from_wait))
            || since_resize >= RESIZE_HARD_STOP;
        if !ready {
            return;
        }

        let after_rect = self.canvas_rect(window);
        let after_frame = self.capture_frame(window);
        let render_size = self.canvas_pixels_for_window(window);
        let uses_design_space = self.canvas_design_size(window).is_some();
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
                // The canvas rect growing only proves the LAYOUT followed the
                // window. The app itself may still be drawing at its old
                // size, with the painter scaling that picture up to fit --
                // which looks blurry and, in a game with a camera, strands
                // the world off-screen. Ask the surface what resolution the
                // app is actually drawing at (K-096).
                // An app that declared a design size IS adapting: it draws in
                // fixed coordinates on purpose and the host scales them to
                // the window, which is the letterboxed answer, not a bug.
                // No canvas2d surface at all means a 3D scene or a widget
                // app: the render-size question does not apply, and asking
                // it failed krate-cubes, which draws through scene3d.
                let redrew_at_new_size = uses_design_space
                    || render_size.is_none()
                    || render_size.is_some_and(|(w, h)| {
                        (w as f32 - after.0).abs() <= 2.0 && (h as f32 - after.1).abs() <= 2.0
                    });
                if grew && !redrew_at_new_size {
                    crate::usability::Observation::broke(format!(
                        "the window grew to {:.0}x{:.0} but the app kept drawing at its old                          size, so the host had to stretch that picture to fit -- a person                          sees a blurry, wrongly-scaled app, and in a game the world falls                          off the screen. Lay out from canvas2d::canvas_size every frame.",
                        after.0, after.1
                    ))
                } else if grew {
                    // The canvas growing proves the LAYOUT followed the
                    // window -- the layout engine does that on its own. The
                    // real question is whether the APP drew into the space
                    // that appeared, and that is what this check missed:
                    // krate-bounce hardcodes 320x240, passed this gate, and
                    // a person resizing it sees the game stranded in a
                    // corner. Look at the new margin for paint (K-096).
                    let painted_margin = uses_design_space
                        || after_frame.as_ref().is_some_and(|frame| {
                            // Compare against the canvas size BEFORE the
                            // resize: anything beyond it is space that only
                            // exists now.
                            frame.content_in_margin(
                                before.0.max(0.0) as u32,
                                before.1.max(0.0) as u32,
                            )
                        });
                    if painted_margin {
                        crate::usability::Observation::Held
                    } else {
                        crate::usability::Observation::broke(format!(
                            "the window grew to {:.0}x{:.0} and the app drew nothing in the                              new space -- its layout is pinned to fixed numbers, so a person                              who resizes the window sees the app stranded in a corner",
                            after.0, after.1
                        ))
                    }
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
        } else if driver.plan.check_keyboard {
            DriveStep::Keyboard
        } else {
            DriveStep::Watch
        }
    }

    /// Deliver a pointer press, then judge the next frame against the one
    /// before it. Runs across two visits, like the resize.
    fn drive_click(&mut self, window: WindowId, from_wait: bool) {
        let clicked_already = self.usability.as_ref().is_some_and(|d| d.action_delivered);
        let before = self.usability.as_ref().and_then(|d| d.before.clone());

        if !clicked_already {
            let Some((x, y, confident, target_widget)) = self.usability_press_target(window) else {
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
                driver.press_delivered_at = Some(std::time::Instant::now());
                driver.clicked_widget = target_widget;
            }
            return;
        }

        // Give the app a real turn before judging it.
        //
        // An event-loop app handles the press like this: `wait` hands it over,
        // the app matches it, drains the rest of the queue with `poll`, and
        // only THEN redraws. Both `wait` and `poll` step this script, so the
        // second visit arrived on that drain -- after the app had matched the
        // press but before it had repainted -- and the capture caught the old
        // frame. A working countdown timer was reported as having a dead Start
        // button on exactly this: its own trace printed "pointer arm entered /
        // widget=7", then the capture, then "rebuild ok running=yes" (K-143).
        //
        // So the judging visit waits for the app to finish its turn. `wait` is
        // that point for an event-loop app -- it has handled everything and is
        // asking for the next event. A frame-loop app (a game) never calls
        // `wait` at all, so it cannot be the only condition: there, a slice of
        // wall clock is the best available signal, and one is granted after
        // `PRESS_SETTLE` regardless. Gating on `wait` alone stalled the script
        // and reported a working app as having closed itself.
        let since_press = self
            .usability
            .as_ref()
            .and_then(|d| d.press_delivered_at)
            .map(|at| at.elapsed())
            .unwrap_or_default();
        // Normally: the app has had its slice AND is back at `wait`, meaning
        // this turn is finished. The second clause is the escape hatch for an
        // app that never calls `wait` -- gating on `wait` alone stalled the
        // script there and reported a working app as having closed itself.
        let ready = (since_press >= PRESS_SETTLE && from_wait) || since_press >= PRESS_GIVE_UP;
        if !ready {
            return;
        }

        let after = self.capture_frame(window);
        let driver = match self.usability.as_mut() {
            Some(driver) => driver,
            None => return,
        };
        let confident = driver.press_was_confident;
        let idle_churn = driver.idle_churn.unwrap_or(0.0);
        driver.action_delivered = false;
        driver.report.click = Some(match (before, after.clone()) {
            (Some(before), Some(after)) => {
                let difference = crate::usability::frame_difference(&before, &after);
                // The press has to move more of the screen than the app moves
                // on its own, or it has proved nothing: a spinner alone used to
                // satisfy this, so a dead button passed on the strength of its
                // own loading animation (K-140). The margin is what separates
                // "something reacted" from "the animation ticked".
                let answered = crate::usability::press_answered(difference, idle_churn);
                if std::env::var_os("KRATE_EVENT_TRACE").is_some() {
                    eprintln!(
                        "krate-check: click difference={difference:.6} idle_churn={idle_churn:.6} \
                         confident={confident} answered={answered} \
                         before={}x{} after={}x{}",
                        before.width, before.height, after.width, after.height
                    );
                }
                if answered {
                    crate::usability::Observation::Held
                } else if idle_churn > 0.0 && !confident {
                    // An animating canvas app that did not visibly react. The
                    // press was a guess at where a control might be, so this
                    // still cannot be called a defect -- but say plainly that
                    // the animation is why, so nobody reads it as a pass.
                    crate::usability::Observation::unobserved(
                        "the app animates on its own, and the press changed no more of the \
                         screen than that animation does, so whether it reacted could not be \
                         told apart from the animation",
                    )
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
        driver.step = if driver.plan.check_keyboard {
            DriveStep::Keyboard
        } else {
            DriveStep::Watch
        };
    }

    /// Focus the app's first control and press Enter, then compare frames
    /// across it (IC-743, test 1548).
    ///
    /// The pointer step proves a control answers a press. This asks whether
    /// the same control answers the keyboard: focus it by its widget id,
    /// deliver Enter down and up, and judge the frame change the way the
    /// pointer step does. A control that answers one and not the other is
    /// unreachable to anyone who cannot use a mouse, and that is a defect of
    /// the app, so it reads as Broke rather than Unobserved.
    ///
    /// A canvas app draws its own controls and exposes no widget to focus, so
    /// there is nothing to send a key to; that is Unobserved, with the reason.
    fn drive_keyboard(&mut self, window: WindowId, from_wait: bool) {
        let delivered_already = self.usability.as_ref().is_some_and(|d| d.action_delivered);
        let before = self.usability.as_ref().and_then(|d| d.before.clone());
        if !delivered_already {
            let clicked = self.usability.as_ref().and_then(|d| d.clicked_widget);
            let clickable: Vec<WidgetId> = self
                .window_placements(window)
                .ok()
                .flatten()
                .map(|(_, placements)| {
                    placements
                        .into_iter()
                        .filter(|p| p.clickable && p.width > 1.0 && p.height > 1.0)
                        .map(|p| p.widget)
                        .collect()
                })
                .unwrap_or_default();
            if clickable.is_empty() {
                if let Some(driver) = self.usability.as_mut() {
                    driver.report.keyboard = Some(crate::usability::Observation::unobserved(
                        "the app draws its own controls, so there is no widget to focus and \
                         no key to send it",
                    ));
                    driver.step = DriveStep::Watch;
                }
                return;
            }
            // Not the control the pointer step just pressed: on a list row a
            // second press selects what is already selected and changes
            // nothing, and that silence would be blamed on the keyboard.
            let Some(widget) = clickable.iter().copied().find(|w| Some(*w) != clicked) else {
                if let Some(driver) = self.usability.as_mut() {
                    driver.report.keyboard = Some(crate::usability::Observation::unobserved(
                        "the only control the host can focus is the one the pointer step \
                         already pressed, and pressing it again changes nothing to compare",
                    ));
                    driver.step = DriveStep::Watch;
                }
                return;
            };
            let dispatcher = self.dispatcher();
            let focused = dispatcher.focus_node(window, widget);
            let key = |pressed: bool| {
                dispatcher.route_key_event(crate::phase3_ui::KeyRouteRequest {
                    window,
                    key: "Enter".to_string(),
                    pressed,
                    modifiers: Default::default(),
                })
            };
            let down = key(true);
            let up = key(false);
            if focused.is_err() || down.is_err() || up.is_err() {
                if let Some(driver) = self.usability.as_mut() {
                    driver.report.keyboard = Some(crate::usability::Observation::unobserved(
                        "this host could not focus a control or deliver a key press",
                    ));
                    driver.step = DriveStep::Watch;
                }
                return;
            }
            if let Some(driver) = self.usability.as_mut() {
                driver.action_delivered = true;
                driver.press_delivered_at = Some(std::time::Instant::now());
            }
            return;
        }
        let since = self
            .usability
            .as_ref()
            .and_then(|d| d.press_delivered_at)
            .map(|at| at.elapsed())
            .unwrap_or_default();
        let ready = (since >= PRESS_SETTLE && from_wait) || since >= PRESS_GIVE_UP;
        if !ready {
            return;
        }
        let after = self.capture_frame(window);
        let driver = match self.usability.as_mut() {
            Some(driver) => driver,
            None => return,
        };
        let idle_churn = driver.idle_churn.unwrap_or(0.0);
        driver.action_delivered = false;
        driver.report.keyboard = Some(match (before, after.clone()) {
            (Some(before), Some(after)) => {
                let difference = crate::usability::frame_difference(&before, &after);
                let answered = crate::usability::press_answered(difference, idle_churn);
                if std::env::var_os("KRATE_EVENT_TRACE").is_some() {
                    eprintln!(
                        "krate-check: keyboard difference={difference:.6} idle_churn={idle_churn:.6} \
                         answered={answered}"
                    );
                }
                if answered {
                    crate::usability::Observation::Held
                } else if idle_churn > 0.0 {
                    crate::usability::Observation::unobserved(
                        "the app animates on its own, and Enter changed no more of the screen \
                         than that animation does, so whether it reacted could not be told apart",
                    )
                } else {
                    crate::usability::Observation::broke(
                        "the app's own control was focused and Enter was pressed, and nothing on \
                         screen changed; a control that answers the pointer but not the keyboard \
                         is unreachable to anyone without a mouse",
                    )
                }
            }
            _ => crate::usability::Observation::unobserved(
                "no frame could be painted around the key press",
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
    /// What is on screen in this window right now, from the widget tree
    /// the app built: every node's kind, its label when it has one, and
    /// where it is. The tree rather than the painted placements, because a
    /// natively lowered control (a text field, say) is not painted by the
    /// shared painter headless and would otherwise be invisible to a task
    /// that types into it. Clicks aim at the rectangle; expectations read
    /// the labels.
    fn on_screen(&self, window: WindowId) -> Vec<OnScreen> {
        let dispatcher = self.dispatcher();
        let Ok(Some(tree)) = dispatcher.widget_tree(window) else {
            return Vec::new();
        };
        let Ok(Some(record)) = dispatcher.window(window) else {
            return Vec::new();
        };
        let Ok(viewport) = LayoutViewport::new(record.size.width as f32, record.size.height as f32)
        else {
            return Vec::new();
        };
        let layout = dispatcher.compute_layout(window, viewport).ok();
        let mut seen: Vec<OnScreen> = tree
            .nodes()
            .iter()
            .map(|(id, node)| OnScreen {
                widget: *id,
                kind: format!("{:?}", node.kind),
                label: node.label.clone().unwrap_or_default(),
                rect: layout
                    .as_ref()
                    .and_then(|layout| absolute_rect(&tree, layout, *id))
                    .map(|r| (r.x, r.y, r.width, r.height)),
            })
            .collect();
        // A canvas app draws its own buttons and text, so the tree holds no
        // labels -- but it tells the screen reader what each one is and where
        // (IC-926). Those are on screen as surely as a widget's label, so a
        // task can press "New note" and expect "Order chairs" in a canvas
        // app too, at the place the app said it drew them. Values count as
        // text on screen (a field's contents), without a place to press.
        let described = self
            .described
            .borrow()
            .get(&window.get())
            .cloned()
            .unwrap_or_default();
        let canvas = seen
            .iter()
            .find(|n| n.kind == format!("{:?}", WidgetKind::Canvas))
            .map(|n| (n.widget, n.rect.unwrap_or_default()));
        if let Some((widget, (ox, oy, _, _))) = canvas {
            for element in described {
                let kind = format!("{:?}", element.role);
                if !element.value.trim().is_empty() {
                    seen.push(OnScreen {
                        widget,
                        kind: kind.clone(),
                        label: element.value.clone(),
                        rect: None,
                    });
                }
                seen.push(OnScreen {
                    widget,
                    kind,
                    label: element.label,
                    rect: Some((
                        ox + element.x,
                        oy + element.y,
                        element.width,
                        element.height,
                    )),
                });
            }
        }
        seen
    }

    /// One turn of the task oracle (IC-743, test 1506): dispatch the next
    /// step, or wait for the last action to settle, or judge an
    /// expectation. Ends with `report.task` set.
    fn drive_task(&mut self, window: WindowId, from_wait: bool) {
        let (index, dispatched_at, step, total, name) = {
            let Some(driver) = self.usability.as_ref() else {
                return;
            };
            let Some(task) = driver.plan.task.as_ref() else {
                return;
            };
            (
                driver.task_index,
                driver.task_dispatched_at,
                task.steps.get(driver.task_index).cloned(),
                task.steps.len(),
                task.name.clone(),
            )
        };
        let trace = std::env::var_os("KRATE_EVENT_TRACE").is_some();
        let finish = |host: &mut Self, outcome: crate::usability::Observation, done: usize| {
            if let Some(driver) = host.usability.as_mut() {
                driver.report.task = Some(outcome);
                driver.report.task_name = Some(name.clone());
                driver.report.task_steps_done = done;
                driver.task_dispatched_at = None;
            }
        };
        let Some(step) = step else {
            if trace {
                eprintln!("krate-check: task {name:?} completed all {total} steps");
            }
            finish(self, crate::usability::Observation::Held, total);
            return;
        };
        let number = index + 1;
        // An action is settling: give the app its frame before the next
        // step, the same grace the click and keyboard steps get.
        if let Some(at) = dispatched_at {
            let held = step.held_key().and_then(Result::ok);
            let look = step.look_motion().and_then(Result::ok);
            let pad = step.pad_press().and_then(Result::ok);
            let stick = step.stick_tilt().and_then(Result::ok);
            let needed = step
                .wait_ms
                .map(std::time::Duration::from_millis)
                .or(held.as_ref().map(|(_, ms)| *ms))
                .or(pad.as_ref().map(|(_, ms)| *ms))
                .or(stick.as_ref().map(|(_, _, ms)| *ms))
                .or(look.as_ref().map(|(_, _, ms)| *ms))
                .unwrap_or(PRESS_SETTLE);
            let since = at.elapsed();
            // A look moves the held pointer evenly over its time.
            if let Some((dx, dy, ms)) = look {
                let frac = (since.as_secs_f32() / ms.as_secs_f32()).min(1.0);
                let given = self
                    .usability
                    .as_ref()
                    .map(|d| d.look_given)
                    .unwrap_or_default();
                let (tx, ty) = (dx * frac, dy * frac);
                self.headless_look(tx - given.0, ty - given.1);
                if let Some(driver) = self.usability.as_mut() {
                    driver.look_given = (tx, ty);
                }
            }
            let ready = (since >= needed && from_wait) || since >= needed.max(PRESS_GIVE_UP);
            if !ready {
                return;
            }
            // A held pad button or stick is let go when its time is up.
            if let Some((button, _)) = pad {
                self.gamepads.borrow_mut().stand_in_press(&button, false);
            }
            if let Some((axis, _, _)) = stick {
                self.gamepads.borrow_mut().stand_in_tilt(&axis, 0.0);
            }
            // A held key is let go when its time is up, before the next step.
            if let Some((key, _)) = held {
                let _ = self
                    .dispatcher()
                    .route_key_event(crate::phase3_ui::KeyRouteRequest {
                        window,
                        key,
                        pressed: false,
                        modifiers: Default::default(),
                    });
            }
            if let Some(driver) = self.usability.as_mut() {
                driver.task_index += 1;
                driver.task_dispatched_at = None;
                driver.report.task_steps_done = number;
            }
            return;
        }
        let on_screen = self.on_screen(window);
        let labels = || {
            let mut seen: Vec<String> = on_screen
                .iter()
                .filter(|n| !n.label.trim().is_empty())
                .map(|n| n.label.clone())
                .collect();
            seen.dedup();
            let shown: Vec<String> = seen.iter().take(24).map(|l| format!("{l:?}")).collect();
            if seen.len() > 24 {
                format!("[{}, ... {} more]", shown.join(", "), seen.len() - 24)
            } else {
                format!("[{}]", shown.join(", "))
            }
        };
        let broke = |host: &mut Self, why: String| {
            if trace {
                eprintln!("krate-check: task {name:?} step {number}: {why}");
            }
            finish(
                host,
                crate::usability::Observation::broke(format!(
                    "task {name:?}, step {number} of {total} ({}): {why}",
                    step.describe()
                )),
                index,
            );
        };
        if let Some(text) = &step.expect_text {
            let wanted = text.trim().to_lowercase();
            if on_screen
                .iter()
                .any(|n| n.label.to_lowercase().contains(&wanted))
            {
                if let Some(driver) = self.usability.as_mut() {
                    driver.task_index += 1;
                    driver.report.task_steps_done = number;
                }
            } else {
                broke(
                    self,
                    format!(
                        "{text:?} is not on screen after the steps before it; on screen: {}",
                        labels()
                    ),
                );
            }
            return;
        }
        if let Some(text) = &step.expect_no_text {
            let unwanted = text.trim().to_lowercase();
            if on_screen
                .iter()
                .any(|n| n.label.to_lowercase().contains(&unwanted))
            {
                broke(
                    self,
                    format!("{text:?} is still on screen; on screen: {}", labels()),
                );
            } else if let Some(driver) = self.usability.as_mut() {
                driver.task_index += 1;
                driver.report.task_steps_done = number;
            }
            return;
        }
        if let Some(path) = &step.shoot {
            let scale = self.screenshot.as_ref().map(|(_, s)| *s).unwrap_or(1.0);
            match self.render_window_png(window, scale, std::path::Path::new(path)) {
                Ok(()) => {
                    if trace {
                        eprintln!("krate-check: task {name:?} step {number}: shot {path}");
                    }
                    if let Some(driver) = self.usability.as_mut() {
                        driver.task_index += 1;
                        driver.report.task_steps_done = number;
                    }
                }
                Err(error) => broke(self, format!("could not write {path:?}: {error}")),
            }
            return;
        }
        // The stand-in controller (IC-936): held now, let go when settled.
        let pad = step.pad_press().and_then(Result::ok);
        let stick = step.stick_tilt().and_then(Result::ok);
        let look = step.look_motion().and_then(Result::ok);
        if step.wait_ms.is_some() || pad.is_some() || stick.is_some() || look.is_some() {
            if let Some(driver) = self.usability.as_mut() {
                driver.look_given = (0.0, 0.0);
            }
            if let Some((button, _)) = &pad {
                self.gamepads.borrow_mut().stand_in_press(button, true);
            }
            if let Some((axis, value, _)) = &stick {
                self.gamepads.borrow_mut().stand_in_tilt(axis, *value);
            }
            if let Some(driver) = self.usability.as_mut() {
                driver.task_dispatched_at = Some(std::time::Instant::now());
            }
            return;
        }
        let dispatcher = self.dispatcher();
        let delivered = if let Some(label) = &step.click {
            let target = on_screen
                .iter()
                .find(|n| n.matches(label, true) && n.rect.is_some())
                .or_else(|| {
                    on_screen
                        .iter()
                        .find(|n| n.matches(label, false) && n.rect.is_some())
                });
            let Some(node) = target else {
                broke(
                    self,
                    format!(
                        "no control labelled {label:?} is on screen; on screen: {}",
                        labels()
                    ),
                );
                return;
            };
            let (rx, ry, rw, rh) = node.rect.unwrap_or_default();
            let (x, y) = (rx + rw / 2.0, ry + rh / 2.0);
            if trace {
                eprintln!(
                    "krate-check: task {name:?} step {number}: pressing widget {} ({label:?}) at {x:.1},{y:.1}",
                    node.widget.get()
                );
            }
            // A host gives focus to what was pressed, so what is typed next
            // goes there.
            let _ = dispatcher.focus_node(window, node.widget);
            let Ok(Some(record)) = dispatcher.window(window) else {
                return;
            };
            let Ok(viewport) =
                LayoutViewport::new(record.size.width as f32, record.size.height as f32)
            else {
                return;
            };
            [true, false].iter().all(|&pressed| {
                dispatcher
                    .route_pointer_event(crate::phase3_ui::PointerRouteRequest {
                        window,
                        viewport,
                        x,
                        y,
                        button: Some(krate_adapter_common::ui::PointerButton::Primary),
                        pressed,
                        modifiers: Default::default(),
                    })
                    .is_ok()
            })
        } else if let Some(Ok((fx, fy))) = step.click_point() {
            let Ok(Some(record)) = dispatcher.window(window) else {
                return;
            };
            let (w, h) = (record.size.width as f32, record.size.height as f32);
            let Ok(viewport) = LayoutViewport::new(w, h) else {
                return;
            };
            let (x, y) = (fx * w, fy * h);
            if trace {
                eprintln!("krate-check: task {name:?} step {number}: pressing the window at {x:.1},{y:.1}");
            }
            [true, false].iter().all(|&pressed| {
                dispatcher
                    .route_pointer_event(crate::phase3_ui::PointerRouteRequest {
                        window,
                        viewport,
                        x,
                        y,
                        button: Some(krate_adapter_common::ui::PointerButton::Primary),
                        pressed,
                        modifiers: Default::default(),
                    })
                    .is_ok()
            })
        } else if let Some(Ok((key, _))) = step.held_key() {
            // Pressed now; released when the step has settled (above).
            dispatcher
                .route_key_event(crate::phase3_ui::KeyRouteRequest {
                    window,
                    key,
                    pressed: true,
                    modifiers: Default::default(),
                })
                .is_ok()
        } else if let Some((key, shift, primary)) = step.key_chord() {
            let modifiers = chord_modifiers(shift, primary);
            [true, false].iter().all(|&pressed| {
                let routed = dispatcher
                    .route_key_event(crate::phase3_ui::KeyRouteRequest {
                        window,
                        key: key.clone(),
                        pressed,
                        modifiers,
                    })
                    .is_ok();
                if pressed {
                    let focused = dispatcher.focused_widget(window).ok().flatten();
                    dispatcher.paste_for_person(window, focused, &key, modifiers);
                }
                routed
            })
        } else if let Some(text) = &step.paste {
            // The run's own clipboard: a check run is headless, so this is
            // Krate's in-process one, never the person's.
            dispatcher.put_on_run_clipboard(text);
            let modifiers = chord_modifiers(false, true);
            let focused = dispatcher.focused_widget(window).ok().flatten();
            [true, false].iter().all(|&pressed| {
                dispatcher
                    .route_key_event(crate::phase3_ui::KeyRouteRequest {
                        window,
                        key: "v".to_string(),
                        pressed,
                        modifiers,
                    })
                    .is_ok()
            }) && dispatcher.paste_for_person(window, focused, "v", modifiers)
        } else if let Some(text) = &step.type_text {
            // Typed text is a text-input event, the way a host delivers it,
            // not one key event per character: an app reads its text field
            // from `Event::TextInput`, and a key named "a" is a key.
            dispatcher
                .route_text_input(crate::phase3_ui::TextInputRouteRequest {
                    window,
                    text: text.clone(),
                })
                .is_ok()
        } else {
            false
        };
        if !delivered {
            broke(self, "this host could not deliver the step".to_string());
            return;
        }
        if let Some(driver) = self.usability.as_mut() {
            driver.task_dispatched_at = Some(std::time::Instant::now());
        }
    }

    pub fn usability_report(&self) -> Option<crate::usability::UsabilityReport> {
        self.usability.as_ref().map(|d| {
            let mut report = d.report.clone();
            report.fit = self.fit_facts();
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

    /// How the app's first canvas lays itself out, for the fit check: the
    /// fixed size it draws in (its own design size, or the window size it
    /// asked for when it never asked its real size), or None when it lays
    /// out from the real canvas. None for an app with no canvas.
    fn fit_facts(&self) -> Option<crate::usability::FitFacts> {
        let canvases = self.canvases.borrow();
        let (&canvas, (window, _, surface)) = canvases.iter().next()?;
        if self.canvases_that_asked.borrow().contains(&canvas)
            || self.screen_windows.borrow().contains(window)
        {
            return Some(crate::usability::FitFacts { fixed: None });
        }
        let explicit = surface
            .design_size()
            .filter(|_| !self.implicit_designs.borrow().contains(&canvas));
        let fixed = explicit.or_else(|| self.requested_sizes.borrow().get(window).copied());
        Some(crate::usability::FitFacts { fixed })
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
        // A usability run has its own, longer deadline. Its stay-open watch runs
        // the app for STAY_OPEN_WATCH (15s) to prove the window does NOT close
        // by itself, and the driver sends its own CloseRequested when the watch
        // ends. The generic headless budget is 5s -- so without widening it here
        // the budget fired first, sent CloseRequested at 5s, the app correctly
        // obeyed, and the watch then reported "the app closed itself after
        // 5.1s". The app did exactly the right thing: it honored a close the
        // RUNTIME sent. A real Apple Music UI written by grok failed its
        // usability stage on precisely this, every time.
        //
        // The budget still fires -- an app must never hang the run forever --
        // but only after the watch has had its full window plus slack, so the
        // watch records the window as held first. After that, this two-strike
        // close is the backstop for an app whose loop ignores the driver's own
        // CloseRequested.
        //
        // A task runs before the watch, so its own scripted time (its waits
        // and held keys, plus the settle every action gets) is added: a game
        // that takes twenty seconds to reach its menu was closed mid-task.
        let budget = if let Some(driver) = self.usability.as_ref() {
            let scripted = driver
                .plan
                .task
                .as_ref()
                .map(|task| task.scripted_time(PRESS_GIVE_UP))
                .unwrap_or_default();
            let driven =
                crate::usability::STAY_OPEN_WATCH + std::time::Duration::from_secs(5) + scripted;
            driven.max(headless_secs_asked().unwrap_or_default())
        } else {
            headless_run_budget()
        };
        if now.duration_since(started) < budget {
            return None;
        }
        // Same two-strike contract as the close button and Ctrl-C: the app is
        // told once and given a grace period to save and leave on its own, and
        // if it is still asking for events after that, the runtime ends the
        // process on its behalf. Without this the budget is only a suggestion,
        // and a game loop that does not match on CloseRequested declines it
        // forever -- which is exactly the hang the budget exists to prevent.
        if let Some(told) = self.headless_close_told.get() {
            if now.duration_since(told) >= HEADLESS_CLOSE_GRACE {
                std::process::exit(0);
            }
            return None;
        }
        self.headless_close_told.set(Some(now));
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
    /// The pixel dimensions the app is really drawing at, from its canvas
    /// surface -- not the rect the layout gave it.
    /// The design size an app fixed for this window's canvas, if any.
    fn canvas_design_size(&self, window: WindowId) -> Option<(f32, f32)> {
        let canvases = self.canvases.borrow();
        canvases
            .values()
            .find(|(w, _, _)| *w == window)
            .and_then(|(_, _, surface)| surface.design_size())
    }

    fn canvas_pixels_for_window(&self, window: WindowId) -> Option<(u32, u32)> {
        let canvases = self.canvases.borrow();
        canvases
            .values()
            .find(|(w, _, _)| *w == window)
            .map(|(_, _, surface)| surface.dimensions())
    }

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
    fn usability_press_target(
        &self,
        window: WindowId,
    ) -> Option<(f32, f32, bool, Option<WidgetId>)> {
        // Aim the driven press at a named point instead of the canvas centre.
        // A canvas app draws its own controls, so centre-of-canvas is a guess
        // that lands on empty space as often as not -- and empty space looks
        // exactly like a dead button. `KRATE_PRESS_AT=x,y` lets a check press
        // a control it knows the position of and get a real verdict.
        if let Some(spec) = std::env::var_os("KRATE_PRESS_AT") {
            let spec = spec.to_string_lossy().to_string();
            let mut parts = spec.split(',');
            if let (Some(x), Some(y)) = (parts.next(), parts.next()) {
                if let (Ok(x), Ok(y)) = (x.trim().parse::<f32>(), y.trim().parse::<f32>()) {
                    return Some((x, y, true, None));
                }
            }
        }
        let (size, placements) = self.window_placements(window).ok()??;
        for placement in &placements {
            if placement.clickable && placement.width > 1.0 && placement.height > 1.0 {
                return Some((
                    placement.x + placement.width / 2.0,
                    placement.y + placement.height / 2.0,
                    true,
                    Some(placement.widget),
                ));
            }
        }
        Some((
            size.width as f32 / 2.0,
            size.height as f32 / 2.0,
            false,
            None,
        ))
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

        // The tree-to-widgets step itself is shared with the browser Player
        // (krate-lowering, IC-956), so the two cannot drift.
        let offsets = self.scroll_offsets.borrow();
        let images = self.images.borrow();
        let placements = krate_lowering::placements(
            &tree,
            &layout,
            |scroll| offsets.get(&(window, scroll)).copied().unwrap_or(0.0),
            |widget| images.get(&(window, widget)).cloned(),
        );
        drop(images);
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
        // No picture of an app its organisation keeps out of pictures (G17).
        if self
            .dispatcher()
            .organisation_blocks_call(krate_policy::SCREEN_CAPTURE_CALL)
        {
            return Err(UiDispatchError::PermissionDenied);
        }
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
            .map_err(|err| UiDispatchError::Layout(format!("write {}: {err}", path.display())))?;
        // What painted the shot, beside it (IC-743, test 1547): a golden
        // comparison must know the renderer, the scale and the colour space
        // it compares, and refuse two shots that differ in any of them. A
        // PNG alone says none of that.
        write_shot_sidecar(
            path,
            &ShotSidecar {
                schema: SHOT_SIDECAR_SCHEMA.to_string(),
                renderer: SHARED_PAINTER.to_string(),
                scale,
                width: pw,
                height: ph,
                logical_width: size.width,
                logical_height: size.height,
                color_space: SHOT_COLOR_SPACE.to_string(),
                os: std::env::consts::OS.to_string(),
                arch: std::env::consts::ARCH.to_string(),
                runtime: env!("CARGO_PKG_VERSION").to_string(),
            },
        )
        .map_err(|err| UiDispatchError::Layout(format!("write {}.json: {err}", path.display())))
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

    /// Put an event into the app's own coordinate system.
    ///
    /// An app that fixed a design size draws in those coordinates, so a
    /// pointer at window (500, 400) means nothing to it. Undo the host's
    /// letterbox mapping here -- the one place every delivered event passes
    /// through -- so hit-testing works with no change in the app (K-096).
    /// The keyboard reaches the same command the pointer does: Enter or
    /// Space pressed on a focused Button, Checkbox, Radio or Switch queues a
    /// press and a release at that control's centre (IC-743, 1548).
    fn synthesize_activation(
        &self,
        dispatcher: &Phase3UiDispatcher<'_>,
        key: &krate_adapter_common::ui::KeyEvent,
    ) {
        if !key.pressed || !matches!(key.key.as_str(), "Enter" | "Space") {
            return;
        }
        let Some(widget) = key.widget else {
            return;
        };
        let Ok(Some(tree)) = dispatcher.widget_tree(key.window) else {
            return;
        };
        // Which controls a key activates, and where, is the shared rule
        // (krate-lowering): the control kinds, and a row of a list.
        let Ok(Some(record)) = dispatcher.window(key.window) else {
            return;
        };
        let Ok(viewport) = LayoutViewport::new(record.size.width as f32, record.size.height as f32)
        else {
            return;
        };
        let Ok(layout) = dispatcher.compute_layout(key.window, viewport) else {
            return;
        };
        let Some((x, y)) = krate_lowering::activation_point(&tree, &layout, widget, &key.key)
        else {
            return;
        };
        if std::env::var_os("KRATE_EVENT_TRACE").is_some() {
            eprintln!(
                "krate-check: {} on focused widget {} synthesized a press at {x:.1},{y:.1}",
                key.key,
                widget.get()
            );
        }
        // Through the dispatcher, exactly as a pointer would arrive: the
        // first version pushed a ready-made WIT event straight to the app,
        // and the frame did not move -- the adapter's own reaction to a
        // press (a list row's selection, a checkbox's tick) is what the
        // pointer step measured, and it never saw the press.
        for pressed in [true, false] {
            let _ = dispatcher.route_pointer_event(crate::phase3_ui::PointerRouteRequest {
                window: key.window,
                viewport,
                x,
                y,
                button: Some(PointerButton::Primary),
                pressed,
                modifiers: key.modifiers,
            });
        }
    }

    fn to_design_space(&self, event: ui::types::Event) -> ui::types::Event {
        let canvases = self.canvases.borrow();
        let Some((_, _, surface)) = canvases.values().next() else {
            return event;
        };
        let Some((dw, dh)) = surface.design_size() else {
            return event;
        };
        let (cw, ch) = surface.dimensions();
        let k = (cw as f32 / dw).min(ch as f32 / dh);
        if k <= 0.0 {
            return event;
        }
        let (ox, oy) = ((cw as f32 - dw * k) / 2.0, (ch as f32 - dh * k) / 2.0);
        let back = |x: f32, y: f32| ((x - ox) / k, (y - oy) / k);
        match event {
            ui::types::Event::Pointer(mut p) => {
                let (x, y) = back(p.x, p.y);
                if std::env::var_os("KRATE_EVENT_TRACE").is_some() {
                    eprintln!(
                        "krate-event: pointer window=({:.1},{:.1}) -> design=({:.1},{:.1}) \
                         canvas={cw}x{ch} design={dw}x{dh} k={k:.3} off=({ox:.1},{oy:.1})",
                        p.x, p.y, x, y
                    );
                }
                p.x = x;
                p.y = y;
                ui::types::Event::Pointer(p)
            }
            ui::types::Event::Wheel(mut w) => {
                let (x, y) = back(w.x, w.y);
                w.x = x;
                w.y = y;
                // Deltas are lengths, not positions: scale, do not offset.
                w.dx /= k;
                w.dy /= k;
                ui::types::Event::Wheel(w)
            }
            other => other,
        }
    }

    fn poll_one_event(&self) -> Result<Option<ui::types::Event>, UiDispatchError> {
        // Anything `key-held` pumped comes out first, in arrival order. It is
        // a real event that simply has not been delivered yet.
        if let Some(event) = self.pending_events.borrow_mut().pop_front() {
            return Ok(Some(self.to_design_space(event)));
        }
        // Through the coalescing pump, not around it: this ran a full native
        // drain per POLL, and a frame loop polls several times a frame. Each
        // drain is a round trip through the window server (~1.5ms), so an
        // idle game paid ~6ms a frame to ask four times whether anything had
        // happened. pump_native_windows collapses those into one per frame.
        self.pump_native_windows();
        let dispatcher = self.dispatcher();
        for window in &self.windows {
            self.sync_native_text(*window, &dispatcher);
        }
        // Files saved on a phone and let go of go to the save sheet.
        let exports = self.chosen_files.borrow_mut().take_ready_exports();
        if let Some(window) = self.windows.first() {
            for path in exports {
                dispatcher.export_file(*window, &path);
            }
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
                // The person's own paste: what they pasted arrives as typed
                // text, so every text field an app draws itself takes a
                // paste (K-1109).
                dispatcher.paste_for_person(sample.window, focused, &sample.key, sample.modifiers);
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
            let moved = {
                let offsets = self.scroll_offsets.borrow();
                krate_lowering::scroll_by(&tree, &layout, sample.x, sample.y, sample.dy, |scroll| {
                    offsets
                        .get(&(sample.window, scroll))
                        .copied()
                        .unwrap_or(0.0)
                })
            };
            if let Some((scroll_id, updated)) = moved {
                self.scroll_offsets
                    .borrow_mut()
                    .insert((sample.window, scroll_id), updated);
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
            // Enter or Space on a focused control IS a press of it (IC-743,
            // test 1548). Every native toolkit activates a focused button on
            // those keys; Krate delivered the raw key and nothing else, so a
            // button that answered the pointer ignored the keyboard in every
            // app that did not special-case Enter -- measured on the
            // hello-gui sample. The app still receives the Key event first;
            // the press and release follow, aimed at the control's centre,
            // exactly as a pointer would have delivered them.
            if let UiEvent::Key(key) = &event {
                self.synthesize_activation(&dispatcher, key);
            }
            // Drops and hovers leave the ordinary path here.
            //
            // Phase 3's `event` variant has no case for either and its WIT is
            // frozen, so `event_to_wit` would drop them on the floor. The
            // conversion from PATH to name-and-token happens right here, at
            // the moment the host learns of the drop, so there is no later
            // point at which a path could reach a guest (K-175).
            match &event {
                UiEvent::FileDropped { path, .. } => {
                    // The capability is checked HERE, where the drop arrives.
                    //
                    // There is no call for an app to make -- the event simply
                    // comes -- so an app that never declared `ui.dropzone`
                    // would otherwise be handed a file and a working token
                    // with nothing asked of it. That is the CP2 promise
                    // inverted: the capability went from declared-and-
                    // unimplemented to implemented-and-unenforced (K-420).
                    //
                    // Refused silently: the app learns nothing, not even that
                    // something was offered. An error it could count would
                    // tell it a file was dragged over a window it has no
                    // right to receive files on.
                    if !self.allows_dropzone(path) {
                        continue;
                    }
                    let name = crate::chosen_files::ChosenFiles::display_name(path);
                    if let Some(token) = self.chosen_files.borrow_mut().remember(path.clone()) {
                        self.pending_drops
                            .borrow_mut()
                            .push_back(DropNotice::Dropped { name, token });
                    }
                    // A drop the token store could not hold (64 files already
                    // open) is silently ignored rather than delivered without
                    // a token: an event naming a file the app cannot open is
                    // worse than no event.
                    continue;
                }
                UiEvent::FileHovering { over, .. } => {
                    // A hover is refused for the same reason. It carries
                    // nothing about the file, but "something is being dragged
                    // over you" is still a fact an app without the capability
                    // has no business learning.
                    // A hover does not name a file, so this asks whether the
                    // app accepts ANYTHING. An app scoped to `image/*` still
                    // lights its target for a drag it may then refuse, which
                    // is the honest behaviour: the system does not tell us
                    // what is being dragged until it lands.
                    if self.allows_dropzone(std::path::Path::new("")) {
                        self.pending_drops
                            .borrow_mut()
                            .push_back(DropNotice::Hovering(*over));
                    }
                    continue;
                }
                _ => {}
            }
            // A window that changed size takes its canvases with it NOW,
            // before the app hears of it and redraws. They used to follow
            // only when the app asked its size (or a tap made the host
            // look), so an app with a design size -- which never asks --
            // drew its first frames for the window it asked for: on an
            // iPhone a calculator sat in the top-left corner until touched.
            if let UiEvent::Resized { id, .. } = &event {
                let bound: Vec<u64> = self
                    .canvases
                    .borrow()
                    .iter()
                    .filter(|(_, (window, _, _))| window == id)
                    .map(|(canvas, _)| *canvas)
                    .collect();
                for canvas in bound {
                    let _ = self.refit_canvas(canvas);
                }
            }
            if let Some(event) = event_to_wit(event) {
                if self.pad_covers(&event) {
                    continue;
                }
                // Count a close request at the one point every real event
                // passes exactly once -- not in poll/wait/key-held, where an
                // event that cycles through the pending queue would be
                // counted twice and a single click would read as two (K-121).
                self.note_close_request(&event);
                return Ok(Some(self.to_design_space(event)));
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
    /// Wait out the rest of the frame budget, so a guest loop that never
    /// sleeps does not spin a core producing frames no display can show.
    ///
    /// The 2D path has its own copy of this inline, wrapped in the canvas-list
    /// and frame-stats branches that only apply there. This is the same wait
    /// for the 3D path, which had none at all.
    fn pace_frame(&self) {
        // An adapter whose own present blocks for the panel IS the pacing;
        // waiting here as well is the second wait that puts input a frame
        // behind.
        if self.dispatcher().supports_canvas_lists() {
            self.last_present.set(Some(std::time::Instant::now()));
            return;
        }
        let due = self
            .last_present
            .get()
            .map(|previous| previous + FRAME_BUDGET)
            .filter(|due| *due > std::time::Instant::now());
        // A threaded app's host is one lock its threads take in turn: waiting
        // here would hold every other thread out of the host for the rest of
        // the frame. The wait happens after the call lets go of it instead.
        #[cfg(not(any(target_os = "ios", target_arch = "wasm32")))]
        if self.screen.pace_outside.get() {
            if let Some(due) = due {
                crate::core_app::pace_until(due);
            }
            self.last_present
                .set(Some(due.unwrap_or_else(std::time::Instant::now)));
            return;
        }
        if let Some(due) = due {
            sleep_until(due);
        }
        self.last_present.set(Some(std::time::Instant::now()));
    }

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
        let scale = self.canvas_backing_scale(window);
        let mut canvases = self.canvases.borrow_mut();
        let Some((_, _, surface)) = canvases.get_mut(&canvas) else {
            return Err(gfx::types::GfxError::InvalidTarget);
        };
        surface
            .resize_scaled(rect.0.max(1.0) as u32, rect.1.max(1.0) as u32, scale)
            .map_err(|error| gfx::types::GfxError::Unsupported(error.to_string()))?;
        Ok(())
    }

    /// Density needed by both the display and an optional screenshot consumer.
    fn canvas_backing_scale(&self, window: WindowId) -> f32 {
        // A worker thread reads the density the main thread last saw: only
        // the window's own thread can ask it (WorkerScreen).
        let native = if self.screen.on_main() {
            let scale = self.dispatcher().window_scale(window);
            self.screen.scales.borrow_mut().insert(window, scale);
            scale
        } else {
            self.screen.used.set(true);
            self.screen
                .scales
                .borrow()
                .get(&window)
                .copied()
                .unwrap_or(1.0)
        };
        // A headless adapter has no display and reports 1x. Raster at the
        // requested capture density as well, otherwise --shoot-scale 2 only
        // enlarges a 1x canvas image. Native runs without a shot are unchanged.
        native.max(
            self.screenshot
                .as_ref()
                .map(|(_, scale)| *scale)
                .unwrap_or(1.0),
        )
    }

    /// Push a canvas's pixels through the image path and re-lower.
    /// Record one draw call into the canvas's display list, when the
    /// adapter renders lists (a GPU backend). The CPU raster still runs
    /// beside the recording; publish decides which consumer presents.
    fn record_op(&self, canvas: u64, op: crate::canvas_list::CanvasOp) {
        // Layout inspection keeps its own copy. The render list below is
        // drained by `publish_canvas` on the adapters that consume it, and
        // is never filled at all on the CPU raster -- so a check that read
        // it would find nothing on desktop and quietly pass everything.
        if self.inspect_layout.get() {
            // First draw after a present starts a new frame. `clear` is not
            // the boundary: most apps never call it, painting a full-canvas
            // gradient instead, and without this their ops from every frame
            // pile up into one list.
            if self.frame_ended.replace(false) {
                self.inspected_ops.borrow_mut().clear();
            }
            self.inspected_ops.borrow_mut().push(op.clone());
        }
        if !self.lists_enabled() {
            return;
        }
        self.canvas_lists
            .borrow_mut()
            .entry(canvas)
            .or_default()
            .ops
            .push(op);
    }

    /// Start recording draw calls for a layout check, and forget any
    /// previous frame.
    ///
    /// Called at the top of each frame: a canvas app repaints continuously,
    /// so ops from earlier frames would pile up and every app would look
    /// like it was drawing on top of itself.
    pub fn begin_layout_inspection(&self) {
        self.inspect_layout.set(true);
        self.inspected_ops.borrow_mut().clear();
    }

    /// The draw calls recorded since the last `begin_layout_inspection`.
    pub fn inspected_ops(&self) -> Vec<crate::canvas_list::CanvasOp> {
        self.inspected_ops.borrow().clone()
    }

    /// Print any text drawn on top of other text in the captured frame.
    ///
    /// Written to stderr in the same shape as the app's own diagnostics, so
    /// a caller that scrapes output sees it without a new channel.
    fn report_text_overlaps(&self) {
        if !self.inspect_layout.get() {
            return;
        }
        let ops = self.inspected_ops.borrow();
        // Measure through a real surface rather than letting the checker
        // guess an average character width. The guess is about twice too
        // wide for punctuation, which made every `"value",` in a JSON view
        // look like the comma was drawn on top of the string before it.
        let canvases = self.canvases.borrow();
        let hits = match canvases.values().next() {
            Some((_, _, surface)) => {
                let measure = |text: &str, size: f32| surface.measure_text(text, size).width;
                crate::text_overlap::find_measured(&ops, Some(&measure))
            }
            None => crate::text_overlap::find(&ops),
        };
        let mut lines: Vec<String> = Vec::new();
        if hits.is_empty() {
            lines.push("layout: no text drawn over other text".to_string());
        } else {
            lines.push(format!(
                "layout: {} place{} where text is drawn over other text",
                hits.len(),
                if hits.len() == 1 { "" } else { "s" }
            ));
            for hit in hits.iter().take(8) {
                lines.push(format!(
                    "layout:   \"{}\" and \"{}\" share {:.0}% of the smaller one, around x {:.0} y {:.0}",
                    hit.first,
                    hit.second,
                    hit.overlap_fraction * 100.0,
                    hit.region.left,
                    hit.region.top,
                ));
            }
        }
        // Dead space: a large part of the window the app drew nothing into.
        if let Some((w, h)) = self.inspected_canvas_size.get() {
            if let Some(dead) = crate::dead_space::find(&ops, w, h) {
                // Worded as what was measured, not as a verdict: this cannot
                // tell an editor with room left to type in from an app that
                // stopped drawing halfway down. See dead_space and K-108.
                lines.push(format!(
                    "layout: nothing is drawn in {:.0}% of the window -- {:.0}x{:.0} at x {:.0} y {:.0}. \
                     If that region is meant to be filled, fill it; if it is somewhere a person types \
                     or drops things, this is fine.",
                    dead.fraction * 100.0,
                    dead.width,
                    dead.height,
                    dead.x,
                    dead.y,
                ));
            }
        }
        // How this frame lands on the screen it was shown on (IC-911):
        // the smallest text as it reaches the glass, and text cut off at the
        // side. check-app runs the app once per screen and reads this back.
        if let Some(path) = std::env::var_os("KRATE_FIT_REPORT") {
            let screen = canvases.values().next().map(|(_, _, surface)| {
                let (w, h) = surface.dimensions();
                (w as f32, h as f32)
            });
            if let (Some(screen), Some(canvas)) = (screen, self.inspected_canvas_size.get()) {
                let facts = match canvases.values().next() {
                    Some((_, _, surface)) => {
                        let measure =
                            |text: &str, size: f32| surface.measure_text(text, size).width;
                        crate::screen_fit::measure(&ops, canvas, screen, Some(&measure))
                    }
                    None => crate::screen_fit::measure(&ops, canvas, screen, None),
                };
                let asked = self.requested_sizes.borrow().values().next().copied();
                // Fixed: the app draws in a size of its own, not the screen's.
                let fixed = (canvas.0 - screen.0).abs() > 1.0 || (canvas.1 - screen.1).abs() > 1.0;
                // Whether the app told the screen reader anything (IC-926).
                let described = self.described.borrow().values().any(|e| !e.is_empty());
                let report = serde_json::json!({
                    "facts": facts,
                    "asked": asked,
                    "fixed": fixed,
                    "described": described,
                });
                let _ = std::fs::write(path, report.to_string());
            }
        }
        for line in &lines {
            eprintln!("{line}");
        }
        // `check-app` runs this in a child process whose stderr is discarded,
        // so findings also go to a file when it names one.
        if let Some(path) = std::env::var_os("KRATE_LAYOUT_REPORT") {
            let _ = std::fs::write(path, lines.join("\n") + "\n");
        }
        // Raw draw list, for working out what a new layout measure can see.
        if let Some(path) = std::env::var_os("KRATE_DRAW_DUMP") {
            let mut out = String::new();
            for op in ops.iter() {
                out.push_str(&crate::text_overlap::describe_op(op));
                out.push('\n');
            }
            let _ = std::fs::write(path, out);
        }
    }

    fn lists_enabled(&self) -> bool {
        if let Some(enabled) = self.lists_enabled.get() {
            return enabled;
        }
        let enabled = self.dispatcher().supports_canvas_lists();
        self.lists_enabled.set(Some(enabled));
        enabled
    }

    fn publish_canvas(&self, canvas: u64) -> Result<(), gfx::types::GfxError> {
        // Record the space the recorded ops are in. This is the frame
        // boundary every app crosses: most never call `clear` at all, and
        // paint a full-canvas gradient instead, so capturing the size there
        // left ten of fourteen generated apps unmeasured.
        //
        // An app that fixed a design size draws in THAT system and the host
        // maps it to the window, so the measure has to use the same one or
        // it compares two different coordinate systems.
        if self.inspect_layout.get() {
            let size = {
                let canvases = self.canvases.borrow();
                canvases.get(&canvas).map(|(_, _, surface)| {
                    surface.design_size().unwrap_or_else(|| {
                        let (w, h) = surface.dimensions();
                        (w as f32, h as f32)
                    })
                })
            };
            if size.is_some() {
                self.inspected_canvas_size.set(size);
            }
        }
        if self.lists_enabled() {
            let taken = {
                let canvases = self.canvases.borrow();
                let Some((window, widget, surface)) = canvases.get(&canvas) else {
                    return Err(gfx::types::GfxError::InvalidTarget);
                };
                let (lw, lh) = surface.dimensions();
                let mut lists = self.canvas_lists.borrow_mut();
                let list = lists.entry(canvas).or_default();
                list.logical_width = lw;
                list.logical_height = lh;
                list.design = surface.design_size();
                let frame = std::mem::take(list);
                (*window, *widget, frame)
            };
            let (window, widget, frame) = taken;
            let handle: krate_adapter_common::ui::CanvasListHandle = std::sync::Arc::new(frame);
            if self
                .dispatcher()
                .present_canvas_list(window, widget, handle)
                .unwrap_or(false)
            {
                return Ok(());
            }
            // The adapter declined this frame; fall through to pixels.
        }
        let stats = std::env::var("KRATE_FRAME_STATS").as_deref() == Ok("1");
        let copy_started = std::time::Instant::now();
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
        let copy_ms = copy_started.elapsed().as_secs_f32() * 1000.0;
        let sync_started = std::time::Instant::now();
        let result = self
            .sync_native_widgets(window)
            .map_err(|error| gfx::types::GfxError::Platform(error.to_string()));
        if stats {
            let sync_ms = sync_started.elapsed().as_secs_f32() * 1000.0;
            let mut times = self.publish_times.borrow_mut();
            times.push((copy_ms, sync_ms));
            if times.len() >= 60 {
                let p = |values: &mut Vec<f32>, q: f32| -> f32 {
                    values.sort_by(|a, b| a.total_cmp(b));
                    values[((values.len() - 1) as f32 * q) as usize]
                };
                let mut copies: Vec<f32> = times.iter().map(|(c, _)| *c).collect();
                let mut syncs: Vec<f32> = times.iter().map(|(_, s)| *s).collect();
                let (fills, text, measure) = crate::canvas_raster::take_op_millis();
                eprintln!(
                    "krate-ops: fills {:.1}ms text {:.1}ms measure {:.1}ms over the last {} frames",
                    fills,
                    text,
                    measure,
                    times.len(),
                );
                eprintln!(
                    "krate-publish: copy p50 {:.2}ms p99 {:.2}ms | sync p50 {:.2}ms p99 {:.2}ms (n={})",
                    p(&mut copies, 0.5),
                    p(&mut copies, 0.99),
                    p(&mut syncs, 0.5),
                    p(&mut syncs, 0.99),
                    times.len(),
                );
                times.clear();
            }
        }
        result
    }

    /// What the window shows now, painted for paper by the same painter a
    /// screenshot uses (`render_window_png`), so the page is what the person
    /// sees -- at `PRINT_SCALE`, about 200 dots per inch on a page.
    pub(crate) fn capture_print_page(
        &self,
        raw: u64,
    ) -> Result<crate::print_host::Page, ui::types::UiError> {
        // No printing where the organisation says so for this app (G17).
        if self
            .dispatcher()
            .organisation_blocks_call(krate_policy::PRINT_CALL)
        {
            return Err(ui::types::UiError::PermissionDenied);
        }
        let window = self.window_id(raw)?;
        let Some((size, placements)) = self
            .window_placements(window)
            .map_err(|err| ui::types::UiError::Platform(err.to_string()))?
        else {
            return Err(ui::types::UiError::Platform(
                "the window has drawn nothing to print yet".to_string(),
            ));
        };
        let scale = crate::print_host::PRINT_SCALE;
        let width = ((size.width as f32) * scale).round().max(1.0) as u32;
        let height = ((size.height as f32) * scale).round().max(1.0) as u32;
        let mut argb = vec![0u32; width as usize * height as usize];
        krate_adapter_common::painter::paint_placements(
            &mut argb,
            width,
            height,
            scale,
            &placements,
            krate_adapter_common::painter::PaintInteraction::default(),
        );
        Ok(crate::print_host::Page {
            width,
            height,
            argb,
            points: (size.width as f32, size.height as f32),
        })
    }

    /// Close every window the app left open, at the end of its run (K-881).
    ///
    /// An app that returns from `run` with a window still open used to leave
    /// that window's adapter session -- and its GPU presenter -- for thread
    /// teardown, where wgpu's queue drop reached for a thread-local wgpu had
    /// already destroyed, and the process aborted after the app had finished
    /// cleanly. Closing here drops them while everything is still alive.
    pub(crate) fn close_open_windows(&mut self) {
        for id in std::mem::take(&mut self.windows) {
            self.maybe_take_screenshot_for(id);
            let _ = self.dispatcher().close_window(id);
        }
    }

    pub(crate) fn print_pages_mut(&mut self) -> &mut Vec<crate::print_host::Page> {
        &mut self.print_pages
    }

    fn window_id(&self, raw: u64) -> Result<WindowId, ui::types::UiError> {
        self.windows
            .iter()
            .copied()
            .find(|window| window.get() == raw)
            .ok_or(ui::types::UiError::InvalidWindow)
    }

    /// Pump once and keep what came out, the way `key-held` does: a game
    /// asking about the mouse every frame may never call `poll`, and its
    /// pointer must still move.
    fn pump_for_query(&self) {
        if let Ok(Some(event)) = self.poll_one_event() {
            self.pending_events.borrow_mut().push_back(event);
        }
    }

    /// The pointer of a window the app owns (IC-909); an empty tracker for a
    /// window it does not.
    pub(crate) fn pointer_state(&self, window: u64) -> krate_adapter_common::ui::PointerTracker {
        self.pump_for_query();
        match self.window_id(window) {
            Ok(id) => self.dispatcher().pointer(id),
            Err(_) => krate_adapter_common::ui::PointerTracker::default(),
        }
    }

    /// Whether a button is held in any of the app's windows.
    pub(crate) fn pointer_button_held(&self, button: PointerButton) -> bool {
        self.pump_for_query();
        let dispatcher = self.dispatcher();
        self.windows
            .iter()
            .any(|window| dispatcher.pointer(*window).held(button))
    }

    /// The fingers on a window the app owns; none for one it does not.
    pub(crate) fn touch_points(&self, window: u64) -> Vec<(u64, f32, f32)> {
        // An app that reads fingers has its own touch controls: no pad.
        self.reads_touch.set(true);
        self.pump_for_query();
        match self.window_id(window) {
            Ok(id) => self.dispatcher().touches(id),
            Err(_) => Vec::new(),
        }
    }

    /// Fit a canvas whose app never asked its size and never set a design
    /// size. Such an app can only be drawing for the size it requested; on a
    /// phone, or after the window was resized, the canvas is another size,
    /// and drawn as-is it sat in a corner, cut off, its taps off target. The
    /// requested size becomes its design size -- uniform scale, centred,
    /// pointer mapped -- exactly what an app gets by asking for one. Made
    /// once, it then fits any screen. Runs at every present.
    fn fit_unasked_canvas(&self, canvas: u64) {
        if self.canvases_that_asked.borrow().contains(&canvas) {
            return;
        }
        let implicit = self.implicit_designs.borrow().contains(&canvas);
        let (window, explicit) = {
            let canvases = self.canvases.borrow();
            let Some((window, _, surface)) = canvases.get(&canvas) else {
                return;
            };
            (*window, surface.design_size().is_some() && !implicit)
        };
        if explicit || self.screen_windows.borrow().contains(&window) {
            return;
        }
        let Some(requested) = self.requested_sizes.borrow().get(&window).copied() else {
            return;
        };
        if self.refit_canvas(canvas).is_err() {
            return;
        }
        let mut canvases = self.canvases.borrow_mut();
        let Some((_, _, surface)) = canvases.get_mut(&canvas) else {
            return;
        };
        let (w, h) = surface.dimensions();
        let differs = (w as f32 - requested.0).abs() > 1.0 || (h as f32 - requested.1).abs() > 1.0;
        if differs && !implicit {
            surface.set_design_size(requested.0, requested.1);
            self.implicit_designs.borrow_mut().insert(canvas);
        } else if !differs && implicit {
            surface.set_design_size(0.0, 0.0);
            self.implicit_designs.borrow_mut().remove(&canvas);
        }
    }

    /// The most picture bytes one run may keep (krate:gfx/canvas-images).
    /// Kept pictures live in the host, outside the app's memory budget, so
    /// they need a bound of their own.
    const KEPT_IMAGE_BYTES: usize = 256 * 1024 * 1024;

    /// Keep a picture for a canvas; the handle, or why not.
    pub(crate) fn keep_image(
        &self,
        canvas: u64,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    ) -> Result<u64, gfx::types::GfxError> {
        if !self.canvases.borrow().contains_key(&canvas) {
            return Err(gfx::types::GfxError::InvalidTarget);
        }
        let image = ImagePixels::new(width, height, rgba)
            .map_err(|error| gfx::types::GfxError::Unsupported(error.to_string()))?;
        let mut kept = self.kept_images.borrow_mut();
        let held: usize = kept.values().map(|i| i.rgba.len()).sum();
        if held + image.rgba.len() > Self::KEPT_IMAGE_BYTES {
            return Err(gfx::types::GfxError::Unsupported(
                "too many kept pictures: forget some first".to_string(),
            ));
        }
        let handle = self.next_kept_image.get();
        self.next_kept_image.set(handle + 1);
        kept.insert((canvas, handle), Arc::new(image));
        Ok(handle)
    }

    /// Draw a kept picture, as draw-pixels (no radii) or draw-pixels-round.
    pub(crate) fn draw_kept_image(
        &self,
        canvas: u64,
        handle: u64,
        area: gfx::types::Rect,
        radii: Option<gfx::types::CornerRadii>,
    ) -> Result<(), gfx::types::GfxError> {
        let Some(image) = self.kept_images.borrow().get(&(canvas, handle)).cloned() else {
            return Err(gfx::types::GfxError::InvalidTarget);
        };
        self.draw_image(canvas, image, area, radii)
    }

    /// Whether a canvas exists.
    pub(crate) fn canvases_contains(&self, canvas: u64) -> bool {
        self.canvases.borrow().contains_key(&canvas)
    }

    /// Draw a picture the host holds -- a kept picture, a video's frame --
    /// as draw-pixels (no radii) or draw-pixels-round.
    pub(crate) fn draw_image(
        &self,
        canvas: u64,
        image: Arc<ImagePixels>,
        area: gfx::types::Rect,
        radii: Option<gfx::types::CornerRadii>,
    ) -> Result<(), gfx::types::GfxError> {
        let rect = (area.x, area.y, area.width, area.height);
        let radii = radii.map(|r| (r.top_left, r.top_right, r.bottom_right, r.bottom_left));
        if self.lists_enabled() || self.inspect_layout.get() {
            self.record_op(
                canvas,
                match radii {
                    Some(radii) => crate::canvas_list::CanvasOp::PixelsRound {
                        rect,
                        radii,
                        image: image.clone(),
                    },
                    None => crate::canvas_list::CanvasOp::Pixels {
                        rect,
                        image: image.clone(),
                    },
                },
            );
        }
        if self.lists_enabled() {
            return Ok(());
        }
        let mut canvases = self.canvases.borrow_mut();
        let Some((_, _, surface)) = canvases.get_mut(&canvas) else {
            return Err(gfx::types::GfxError::InvalidTarget);
        };
        match radii {
            Some(radii) => {
                surface.draw_pixels_round(area.x, area.y, area.width, area.height, radii, &image)
            }
            None => surface.draw_pixels(area.x, area.y, area.width, area.height, &image),
        }
        Ok(())
    }

    /// Let a kept picture go.
    pub(crate) fn forget_image(&self, canvas: u64, handle: u64) {
        self.kept_images.borrow_mut().remove(&(canvas, handle));
    }

    /// Tell the host the app adapts to any screen (see `set_adapts` on the
    /// adapter), before it opens a window.
    pub fn set_adapts(&self, adapts: bool) {
        self.dispatcher().set_adapts(adapts);
    }

    /// Follow the on-screen pad (IC-914): shown on a touch screen for an app
    /// that asked about keys and never read touch, built from those keys,
    /// pressed by the fingers on it. A key a finger presses or lifts is also
    /// sent as a key event, for an app that listens for events.
    fn update_pad(&self) {
        let Some(window) = self.windows.first().copied() else {
            return;
        };
        let screen = self.screen_now(window);
        let wanted = !self.reads_touch.get() && screen.is_some_and(|(_, _, touch, _)| touch);
        let built = if wanted {
            screen.and_then(|(w, h, _, safe)| {
                let pad = crate::touch_pad::Pad::from_keys(&self.keys_asked.borrow())?;
                let layout = crate::touch_pad::layout(&pad, w, h, safe);
                Some((pad, layout))
            })
        } else {
            None
        };
        let now = match &built {
            Some((pad, layout)) => {
                let mut fingers: Vec<(f32, f32)> = self
                    .dispatcher()
                    .touches(window)
                    .into_iter()
                    .map(|(_, x, y)| (x, y))
                    .collect();
                // `KRATE_PAD_FINGERS=x,y;x,y` (headless): fingers held down,
                // to draw the pad being pressed.
                if self.headless {
                    if let Ok(spec) = std::env::var("KRATE_PAD_FINGERS") {
                        fingers.extend(spec.split(';').filter_map(|pair| {
                            let (x, y) = pair.split_once(',')?;
                            Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
                        }));
                    }
                }
                if fingers
                    .iter()
                    .any(|&(x, y)| crate::touch_pad::covers(layout, x, y))
                {
                    self.pad_touched_at.set(std::time::Instant::now());
                }
                let held = crate::touch_pad::held(pad, layout, &fingers);
                *self.pad_fingers.borrow_mut() = fingers;
                held
            }
            None => std::collections::BTreeSet::new(),
        };
        let before = self.pad_held.replace(now.clone());
        let key_event = |key: &String, pressed: bool| {
            ui::types::Event::Key(ui::types::KeyEvent {
                window: window.get(),
                widget: None,
                key: key.clone(),
                pressed,
                modifiers: modifiers_to_wit(Modifiers::default()),
            })
        };
        let mut pending = self.pending_events.borrow_mut();
        for key in now.difference(&before) {
            pending.push_back(key_event(key, true));
        }
        for key in before.difference(&now) {
            pending.push_back(key_event(key, false));
        }
        drop(pending);
        *self.pad.borrow_mut() = built;
    }

    /// Whether a pointer event landed on the pad.
    fn pad_covers(&self, event: &ui::types::Event) -> bool {
        let ui::types::Event::Pointer(p) = event else {
            return false;
        };
        self.pad
            .borrow()
            .as_ref()
            .is_some_and(|(_, layout)| crate::touch_pad::covers(layout, p.x, p.y))
    }

    /// Draw the pad over the frame about to be shown, in the canvas's own
    /// coordinates (a game with a design size is scaled; the pad must not
    /// be).
    /// Returns the app's design size when it switched it off for the pad,
    /// to be put back once the frame is published.
    fn draw_pad(&mut self, canvas: u64) -> Option<(f32, f32)> {
        use gfx::canvas2d::Host as _;
        self.update_pad();
        let (pad, layout) = self.pad.borrow().clone()?;
        // Drawn in the screen's own points, outside the app's design: a
        // fixed-size game is scaled into the middle of a phone, and a pad
        // drawn in its coordinates was cut off over the side bands. The list
        // carries a marker; the CPU raster has its design switched off for
        // the pad's ops and back on after.
        // The GPU path (a recorded list) only needs the marker. The CPU
        // raster has its bands painted first, then its design switched off
        // for the pad and back on once the frame is published.
        let design = if self.lists_enabled() {
            None
        } else {
            self.canvases
                .borrow()
                .get(&canvas)
                .and_then(|(_, _, surface)| surface.design_size())
        };
        self.record_op(canvas, crate::canvas_list::CanvasOp::ScreenSpace(true));
        if design.is_some() {
            if let Some((_, _, surface)) = self.canvases.borrow_mut().get_mut(&canvas) {
                surface.bake_letterbox();
                surface.set_design_size(0.0, 0.0);
            }
        }
        let color = |r: f32, g: f32, b: f32, a: f32| gfx::types::Color { r, g, b, a };
        let pt = |x: f32, y: f32| gfx::types::Point { x, y };
        let held = self.pad_held.borrow().clone();
        let pressed = |keys: &[String]| keys.iter().any(|key| held.contains(key));
        // Left alone for a few seconds the pad rests dimmer, so the game
        // shows through; a touch brings it back at once.
        let idle = self.pad_touched_at.get().elapsed().as_secs_f32();
        let fade = if idle < 2.5 {
            1.0
        } else {
            1.0 - ((idle - 2.5) / 0.6).min(1.0) * 0.45
        };
        let white = |a: f32| color(1.0, 1.0, 1.0, a * fade);
        let shadow = |a: f32| color(0.0, 0.0, 0.0, a * fade);
        // A rounded stroke from (x0, y0) to (x1, y1), as a run of dots: the
        // canvas has no line, and no font has to carry an arrow.
        let stroke = |host: &mut Self, x0: f32, y0: f32, x1: f32, y1: f32, w: f32, ink| {
            let len = ((x1 - x0).powi(2) + (y1 - y0).powi(2)).sqrt();
            let steps = (len / (w * 0.35)).ceil().max(1.0) as usize;
            for i in 0..=steps {
                let t = i as f32 / steps as f32;
                let _ = host.fill_circle(
                    canvas,
                    pt(x0 + (x1 - x0) * t, y0 + (y1 - y0) * t),
                    w / 2.0,
                    ink,
                );
            }
        };
        let bold = gfx::types::TextStyle {
            weight: 700,
            italic: false,
            letter_spacing: 0.0,
            family: gfx::types::FontFamily::Sans,
        };

        // The stick: a glass base, a chevron for each way the game reads, and
        // a knob that follows the thumb.
        if let Some((cx, cy, r)) = layout.dpad {
            let _ = self.fill_circle(canvas, pt(cx, cy + 2.0), r + 1.0, shadow(0.18));
            let _ = self.fill_circle(canvas, pt(cx, cy), r, shadow(0.32));
            let _ = self.fill_circle(canvas, pt(cx, cy), r, white(0.06));
            let _ = self.stroke_circle(canvas, pt(cx, cy), r - 0.75, 1.5, white(0.22));
            for (dir, keys) in &pad.dirs {
                let on = pressed(keys);
                // The chevron's tip, pointing out, and its two arms.
                let (ux, uy) = match dir {
                    crate::touch_pad::Dir::Left => (-1.0, 0.0),
                    crate::touch_pad::Dir::Right => (1.0, 0.0),
                    crate::touch_pad::Dir::Up => (0.0, -1.0),
                    crate::touch_pad::Dir::Down => (0.0, 1.0),
                };
                let tip = r * 0.80;
                let back = r * 0.66;
                let spread = r * 0.13;
                let (tx, ty) = (cx + ux * tip, cy + uy * tip);
                let (bx, by) = (cx + ux * back, cy + uy * back);
                let (px, py) = (-uy * spread, ux * spread);
                let ink = if on { white(1.0) } else { white(0.55) };
                if on {
                    let _ = self.radial_gradient(
                        canvas,
                        pt(cx + ux * r * 0.62, cy + uy * r * 0.62),
                        r * 0.55,
                        white(0.28),
                        white(0.0),
                    );
                }
                let w = (r * 0.065).max(3.0);
                stroke(self, bx + px, by + py, tx, ty, w, ink);
                stroke(self, bx - px, by - py, tx, ty, w, ink);
            }
            let (kx, ky) =
                crate::touch_pad::knob(&layout, &self.pad_fingers.borrow()).unwrap_or((cx, cy));
            let kr = r * 0.42;
            let touched = (kx, ky) != (cx, cy) || pad.dirs.iter().any(|(_, keys)| pressed(keys));
            let _ = self.fill_circle(canvas, pt(kx, ky + 3.0), kr + 1.0, shadow(0.30));
            let _ = self.fill_circle(
                canvas,
                pt(kx, ky),
                kr,
                white(if touched { 0.92 } else { 0.78 }),
            );
            let _ = self.stroke_circle(
                canvas,
                pt(kx, ky),
                kr - 0.5,
                1.0,
                color(1.0, 1.0, 1.0, fade),
            );
        }

        // The buttons: glass circles, labelled; pressed, they fill white
        // and their label turns dark.
        for ((text, keys), (bx, by, br)) in pad.buttons.iter().zip(&layout.buttons) {
            let on = pressed(keys);
            let r = if on { br * 1.06 } else { *br };
            let _ = self.fill_circle(canvas, pt(*bx, by + 2.0), r + 1.0, shadow(0.18));
            if on {
                let _ = self.fill_circle(canvas, pt(*bx, *by), r, white(0.92));
            } else {
                let _ = self.fill_circle(canvas, pt(*bx, *by), r, shadow(0.32));
                let _ = self.fill_circle(canvas, pt(*bx, *by), r, white(0.08));
                let _ = self.stroke_circle(canvas, pt(*bx, *by), r - 0.75, 1.5, white(0.30));
            }
            let size = if text.chars().count() > 2 {
                br * 0.40
            } else {
                br * 0.62
            };
            let width = self
                .measure_text_styled(canvas, text.to_string(), size, bold)
                .ok()
                .and_then(|m| m.ok())
                .map(|m| m.width)
                .unwrap_or(0.0);
            let ink = if on {
                color(0.08, 0.09, 0.12, fade)
            } else {
                white(0.92)
            };
            let _ = self.draw_text_styled(
                canvas,
                text.to_string(),
                pt(bx - width / 2.0, by + size * 0.36),
                size,
                ink,
                bold,
            );
        }
        self.record_op(canvas, crate::canvas_list::CanvasOp::ScreenSpace(false));
        design
    }

    /// Publish a frame: pace it, then hand it to whichever consumer shows it.
    fn present_frame(&mut self, canvas: u64) -> wasmtime::Result<Result<(), gfx::types::GfxError>> {
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
        // Same for the close button: a game loop may never call poll or wait,
        // so the second unanswered close click has to be honoured here or the
        // window cannot be shut at all (K-121).
        if self.close_ignored_by_guest() {
            return Err(crate::end_unanswered_close());
        }
        // A GPU adapter's present blocks for the panel inside
        // get_current_texture, and that IS the pacing. Adding our own timer
        // on top made the guest free-run into FIFO's queue: measured on
        // device as present-done immediately followed by present-start,
        // every frame landing one refresh late, a hard 30 fps against a
        // 60 Hz link, and acquire pinned at 27 ms. Let the display pace it.
        // Keep the canvas the same size as its window, even for an app that
        // never calls canvas-size. Without this the app keeps drawing at its
        // opening size and the host scales that picture up to fit: blurry
        // text, and a game's world pushed off the edge (K-096). Refitting
        // here means the very next frame is drawn at the real resolution.
        let _ = self.refit_canvas(canvas);
        if self.lists_enabled() {
            self.last_present.set(Some(std::time::Instant::now()));
            return Ok(self.publish_canvas(canvas));
        }
        // A new frame begins: the next poll must see the queue for real.
        self.last_pump.set(None);
        let entry = std::time::Instant::now();
        let gap_ms = self
            .last_present_entry
            .get()
            .map(|t| t.elapsed().as_secs_f32() * 1000.0)
            .unwrap_or(0.0);
        self.last_present_entry.set(Some(entry));
        let mut slept_ms = 0.0f32;
        if let Some(previous) = self.last_present.get() {
            let elapsed = previous.elapsed();
            if elapsed < FRAME_BUDGET {
                let remainder = FRAME_BUDGET - elapsed;
                // Prefer the display's own clock where the adapter has one:
                // park until the panel says a frame is due (CADisplayLink on
                // iOS), capped by our budget so a host without a link still
                // paces itself. Sleeping a fixed 16 ms drifts against the
                // real refresh; on a 120 Hz phone that drift is visible.
                //
                // NOT the plain input-waking park of an earlier attempt:
                // that let a 120 Hz finger present at touch rate, exhaust
                // the drawable pool, and stall the next acquire for its full
                // one-second timeout. Waking on VSYNC is bounded by the
                // panel; waking on touches is not.
                // Only sleep when the adapter does NOT block for vsync
                // itself. A GPU adapter's present already waits for the
                // panel inside get_current_texture, so pacing here as well
                // is the second wait that put input a frame behind
                // (measured: 12 ms of a 15.3 ms frame in acquire).
                if !self.dispatcher().supports_canvas_lists() {
                    let sleep_started = std::time::Instant::now();
                    // Sleep short, then close the gap by yielding. Every OS
                    // rounds a sleep UP to its timer granularity, and on
                    // Windows that default is ~15.6ms -- a 2ms request
                    // costing a full tick, twice a frame, is how 6ms of
                    // work became 14fps. Asking for 1ms resolution fixes
                    // the scale of the error; landing the last slice by
                    // yielding fixes the error itself, on every platform.
                    const SPIN_MARGIN: std::time::Duration =
                        std::time::Duration::from_micros(1_200);
                    if remainder > SPIN_MARGIN {
                        std::thread::sleep(remainder - SPIN_MARGIN);
                    }
                    while previous.elapsed() < FRAME_BUDGET {
                        std::thread::yield_now();
                    }
                    slept_ms = sleep_started.elapsed().as_secs_f32() * 1000.0;
                }
            }
        }
        if std::env::var("KRATE_FRAME_STATS").as_deref() == Ok("1") {
            let mut times = self.present_times.borrow_mut();
            times.push((gap_ms, slept_ms));
            if times.len() >= 60 {
                let p = |values: &mut Vec<f32>, q: f32| -> f32 {
                    values.sort_by(|a, b| a.total_cmp(b));
                    values[((values.len() - 1) as f32 * q) as usize]
                };
                let mut gaps: Vec<f32> = times.iter().map(|(g, _)| *g).collect();
                let mut sleeps: Vec<f32> = times.iter().map(|(_, sl)| *sl).collect();
                let pump = PUMP_MS.with(|c| c.replace(0.0));
                let calls = PUMP_CALLS.with(|c| c.replace(0));
                eprintln!("krate-pump: {pump:.1}ms across {calls} pump calls over these frames");
                eprintln!(
                    "krate-present: gap p50 {:.2}ms p99 {:.2}ms | slept p50 {:.2}ms p99 {:.2}ms (n={})",
                    p(&mut gaps, 0.5),
                    p(&mut gaps, 0.99),
                    p(&mut sleeps, 0.5),
                    p(&mut sleeps, 0.99),
                    times.len(),
                );
                times.clear();
            }
        }
        self.last_present.set(Some(std::time::Instant::now()));

        let result = self.publish_canvas(canvas);
        // A presented canvas is a real drawn frame -- the right moment to
        // capture a 2D game or drawing, which may never call wait.
        if result.is_ok() {
            self.maybe_take_screenshot();
        }
        // The measured frame is done with; the next draw op begins another.
        self.frame_ended.set(true);
        self.end_stepped_frame();
        Ok(result)
    }

    /// One step of the scroll drive (`KRATE_DRIVE_SCROLL`): a scroll event
    /// at the middle of the first window, down for the first half of the
    /// steps and back up for the second; when the steps are done, the cost
    /// report is written (`KRATE_COST_REPORT`) and the window asked to
    /// close. None when no drive was asked for, or not headless.
    fn drive_scroll_step(&self) -> Option<ui::types::Event> {
        if !self.headless {
            return None;
        }
        let left = self.drive_scroll_left.get()?;
        let window = *self.windows.first()?;
        if left == 0 {
            self.drive_scroll_left.set(None);
            self.write_cost_report();
            return Some(ui::types::Event::CloseRequested(window.get()));
        }
        self.drive_scroll_left.set(Some(left - 1));
        let (w, h) = self
            .dispatcher()
            .window(window)
            .ok()
            .flatten()
            .map(|record| (record.size.width as f32, record.size.height as f32))
            .unwrap_or((400.0, 800.0));
        let down = left > self.drive_scroll_total / 2;
        self.guest_since.set(Some(std::time::Instant::now()));
        Some(ui::types::Event::Wheel(ui::types::WheelEvent {
            window: window.get(),
            widget: None,
            x: w / 2.0,
            y: h / 2.0,
            dx: 0.0,
            dy: if down { 24.0 } else { -24.0 },
            modifiers: modifiers_to_wit(Modifiers::default()),
        }))
    }

    /// The frame costs the scroll drive measured, as JSON: how many frames
    /// answered, and the median, 95th percentile and worst, in ms.
    fn write_cost_report(&self) {
        let Some(path) = std::env::var_os("KRATE_COST_REPORT") else {
            return;
        };
        let mut costs = self.frame_costs.borrow().clone();
        costs.sort_by(|a, b| a.total_cmp(b));
        let at = |q: f32| -> f32 {
            if costs.is_empty() {
                0.0
            } else {
                costs[((costs.len() - 1) as f32 * q).round() as usize]
            }
        };
        let report = serde_json::json!({
            "steps": self.drive_scroll_total,
            "frames": costs.len(),
            "p50_ms": at(0.5),
            "p95_ms": at(0.95),
            "max_ms": costs.last().copied().unwrap_or(0.0),
            "first_frame_ms": self.first_wait_ms.get(),
        });
        let _ = std::fs::write(path, report.to_string());
    }

    /// A window's screen (krate:ui/screen, IC-912): width and height in
    /// points, a finger or a mouse, and the edges something else covers
    /// (top, right, bottom, left). None for a window the app does not own.
    ///
    /// Headless with `KRATE_SCREEN` set to a phone's size, the window
    /// answers as an iPhone 13 Pro running an app does -- touch, the notch
    /// and home bar, Krate's close button at the top right -- so check-app
    /// judges an app on the phone it will meet, not on a phone-shaped
    /// desktop window.
    /// A window's screen, without counting it as the app asking (see
    /// `screen_facts`).
    fn screen_now(&self, id: WindowId) -> Option<ScreenAnswer> {
        let record = self.dispatcher().window(id).ok().flatten()?;
        let (w, h) = (record.size.width as f32, record.size.height as f32);
        let mut kind = self.dispatcher().screen_kind(id);
        if self.headless && std::env::var_os("KRATE_SCREEN").is_some() && w.min(h) < 600.0 {
            kind.touch = true;
            kind.safe = if h >= w {
                (89.0, 0.0, 34.0, 0.0)
            } else {
                (50.0, 47.0, 21.0, 47.0)
            };
        }
        Some((w, h, kind.touch, kind.safe))
    }

    pub(crate) fn screen_facts(&self, window: u64) -> Option<ScreenAnswer> {
        let id = self.window_id(window).ok()?;
        // Asking is laying out from the screen: drop any design the host
        // chose for this window's canvases (see `fit_unasked_canvas`).
        if !self.screen_windows.borrow().contains(&id) {
            self.screen_windows.borrow_mut().push(id);
            let mut canvases = self.canvases.borrow_mut();
            for (canvas, (bound, _, surface)) in canvases.iter_mut() {
                if *bound == id && self.implicit_designs.borrow_mut().remove(canvas) {
                    surface.set_design_size(0.0, 0.0);
                }
            }
        }
        let record = self.dispatcher().window(id).ok().flatten()?;
        let (w, h) = (record.size.width as f32, record.size.height as f32);
        let mut kind = self.dispatcher().screen_kind(id);
        if self.headless && std::env::var_os("KRATE_SCREEN").is_some() && w.min(h) < 600.0 {
            kind.touch = true;
            kind.safe = if h >= w {
                (89.0, 0.0, 34.0, 0.0)
            } else {
                (50.0, 47.0, 21.0, 47.0)
            };
        }
        Some((w, h, kind.touch, kind.safe))
    }

    /// The on-screen keyboard for a window the app owns; never for one it
    /// does not.
    pub(crate) fn text_entry(&self, window: u64, on: bool) -> bool {
        match self.window_id(window) {
            Ok(id) => self.dispatcher().set_text_entry(id, on),
            Err(_) => false,
        }
    }

    pub(crate) fn pointer_take_motion(&self, window: u64) -> (f32, f32) {
        if self.scripted_pointer() {
            let mut pointers = self.headless_pointer.borrow_mut();
            return pointers
                .get_mut(&window)
                .map(|(_, motion)| std::mem::take(motion))
                .unwrap_or((0.0, 0.0));
        }
        self.pump_for_query();
        match self.window_id(window) {
            Ok(id) => self.dispatcher().take_pointer_motion(id),
            Err(_) => (0.0, 0.0),
        }
    }

    pub(crate) fn pointer_capture(
        &self,
        window: u64,
        capture: bool,
    ) -> Result<(), ui::types::UiError> {
        let id = self.window_id(window)?;
        // Headless there is no screen to hold a cursor on. A driven run has a
        // script to supply raw motion, so there holding is a real promise; a
        // plain headless run has nothing to move it and still refuses, rather
        // than claim to hold a pointer that does not exist.
        if self.scripted_pointer() {
            let _ = id;
            let mut pointers = self.headless_pointer.borrow_mut();
            let entry = pointers.entry(window).or_insert((false, (0.0, 0.0)));
            entry.0 = capture;
            if !capture {
                entry.1 = (0.0, 0.0);
            }
            return Ok(());
        }
        self.dispatcher()
            .set_pointer_capture(id, capture)
            .map_err(dispatch_error_to_ui_error)
    }

    /// Whether the app holds the pointer in a window.
    pub(crate) fn pointer_captured(&self, window: u64) -> bool {
        if self.scripted_pointer() {
            return self
                .headless_pointer
                .borrow()
                .get(&window)
                .is_some_and(|(held, _)| *held);
        }
        self.pointer_state(window).captured
    }

    /// Headless and driven: the script is the mouse (IC-937).
    fn scripted_pointer(&self) -> bool {
        self.headless && self.usability.is_some()
    }

    /// Give a held pointer raw motion (a `look` step). A pointer the app does
    /// not hold moves nothing, as a free cursor would not turn a camera.
    fn headless_look(&self, dx: f32, dy: f32) {
        for (held, motion) in self.headless_pointer.borrow_mut().values_mut() {
            if *held {
                motion.0 += dx;
                motion.1 += dy;
            }
        }
    }
}

// Accumulated milliseconds inside the native event pump, reported with the
// present stats. Between-frame time has to be attributable, or "it lags"
// stays a guess.
thread_local! {
    static PUMP_MS: std::cell::Cell<f32> = const { std::cell::Cell::new(0.0) };
}

struct PumpTimer(std::time::Instant);
impl Drop for PumpTimer {
    fn drop(&mut self) {
        let ms = self.0.elapsed().as_secs_f32() * 1000.0;
        PUMP_MS.with(|c| c.set(c.get() + ms));
        PUMP_CALLS.with(|c| c.set(c.get() + 1));
    }
}

thread_local! {
    static PUMP_CALLS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// One word per event kind, for KRATE_EVENT_TRACE diagnostics.
fn event_name(event: &ui::types::Event) -> &'static str {
    use ui::types::Event as E;
    match event {
        E::CloseRequested(_) => "close",
        E::Resized(_) => "resized",
        E::RedrawRequested(_) => "redraw",
        E::Pointer(_) => "pointer",
        E::Key(_) => "key",
        E::TextInput(_) => "text",
        E::TextChanged(_) => "text-changed",
        E::Action(_) => "action",
        E::FocusChanged(_) => "focus",
        E::ThemeChanged(_) => "theme",
        E::Wheel(_) => "wheel",
    }
}

/// Nearest Scroll ancestor of a widget, if any.
// The scroll and focus rules moved to krate-lowering with the rest of the
// tree-to-widgets step (IC-956); the tests below still hold them here.
#[cfg(test)]
use krate_lowering::clamped_scroll_offset;
use krate_lowering::press_focuses;

fn widget_id(raw: u64) -> Result<WidgetId, ui::types::UiError> {
    WidgetId::new(raw).map_err(|_| ui::types::UiError::InvalidWidget)
}

pub(crate) fn widget_kind_from_wit(kind: ui::types::WidgetKind) -> WidgetKind {
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
    let kind = widget_kind_from_wit(node.kind);
    widget_node_from_parts(
        node.id,
        node.parent,
        kind,
        node.label,
        node.role,
        WidgetStyle {
            width: node.style.width,
            height: node.style.height,
            grow: node.style.grow,
            padding: node.style.padding,
            // Phase 3's `style` has neither a text block nor a placement. An
            // app on the frozen phase gets the host's ordinary label style and
            // the corner it has always started from, exactly as it did.
            text: None,
            place: krate_adapter_common::ui::Placement::Default,
            // Nor a box: a Phase 3 app gets the host's own look, unchanged.
            r#box: None,
        },
        node.checked,
        node.value,
        node.selected,
        node.text_cursor.map(|tc| (tc.cursor, tc.anchor)),
    )
}

/// Build and validate a widget node from parts already in the host's own
/// types.
///
/// Both phases land here. Phase 4's `widget-kind` has a case Phase 3's does
/// not, so the two cannot share a WIT record -- but they share every RULE,
/// which is what this function holds. Splitting it this way is what keeps the
/// frozen phase and the current one from drifting on validation (K-403).
#[allow(clippy::too_many_arguments)]
impl Phase3GuiHost {
    /// Take the next drop or hover, if one is waiting.
    ///
    /// Phase 4's events implementation calls this before delegating, so a drop
    /// is delivered ahead of the ordinary queue rather than behind a backlog
    /// of pointer moves.
    /// Whether this app declared `ui.dropzone` and was granted it.
    ///
    /// Asked once per drop rather than cached: a grant can be revoked during
    /// a run, and a capability that was checked at startup would keep letting
    /// files in afterwards.
    fn allows_dropzone(&self, path: &std::path::Path) -> bool {
        self.runtime
            .guard()
            .check(&UapiCall::Ui(UiCall::Dropzone {
                mime: mime_for_dropped_path(path),
            }))
            .is_ok()
    }

    pub(crate) fn take_drop_notice(&self) -> Option<DropNotice> {
        self.inject_test_drop();
        self.pending_drops.borrow_mut().pop_front()
    }

    /// Deliver a drop named by `KRATE_TEST_DROP`, once.
    ///
    /// A drop cannot be driven from a `--shoot`: it needs a hand on a mouse,
    /// and a capability nobody can test without one is a capability nobody
    /// tests. This injects exactly what a real drop injects -- a hover, then
    /// the file -- through the same path, so what it proves is the real path
    /// and not a parallel one.
    ///
    /// The path still becomes a token here, so even the test cannot hand a
    /// guest a path (K-175).
    fn inject_test_drop(&self) {
        if self.test_drop_done.get() {
            return;
        }
        let Some(raw) = std::env::var_os("KRATE_TEST_DROP") else {
            self.test_drop_done.set(true);
            return;
        };
        self.test_drop_done.set(true);
        let path = std::path::PathBuf::from(raw);
        // Through the SAME gate a real drop passes.
        //
        // The first version of this wrote straight into the queue, which made
        // the hook a hole in the capability check it was meant to help test:
        // an app with no `ui.dropzone` received the injected file. A test hook
        // that skips the thing under test is worse than no hook.
        if !self.allows_dropzone(&path) {
            return;
        }
        let name = crate::chosen_files::ChosenFiles::display_name(&path);
        let mut queue = self.pending_drops.borrow_mut();
        queue.push_back(DropNotice::Hovering(true));
        if let Some(token) = self.chosen_files.borrow_mut().remember(path) {
            queue.push_back(DropNotice::Dropped { name, token });
        }
        queue.push_back(DropNotice::Hovering(false));
    }
}

/// The mime type a dropped path is treated as, for the capability check.
///
/// Deliberately small and by extension only. The alternative is sniffing the
/// file's contents, which means READING a file before deciding whether the
/// app may have it -- the wrong order. An extension is what the system itself
/// uses to pick an icon, and an app that wants everything declares `*`.
///
/// Anything unrecognised is `application/octet-stream`, the standard name for
/// "bytes we will not characterise". An app scoped to `image/*` does not get
/// it; an app scoped to `*` does.
fn mime_for_dropped_path(path: &std::path::Path) -> String {
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let mime = match ext.as_str() {
        "txt" | "log" | "md" | "csv" => "text/plain",
        "html" | "htm" => "text/html",
        "json" => "application/json",
        "xml" => "application/xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "bmp" => "image/bmp",
        "wav" => "audio/wav",
        "mp3" => "audio/mpeg",
        "ogg" => "audio/ogg",
        "flac" => "audio/flac",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    };
    mime.to_string()
}

/// A drop or a hover, in the form a guest may see it.
///
/// Never carries a path. A `Dropped` holds the file's own name and an opaque
/// token for `fs.files.open-chosen`; a `Hovering` holds nothing at all,
/// because a hover is not consent and reveals nothing about the file.
#[derive(Debug, Clone)]
pub(crate) enum DropNotice {
    Dropped { name: String, token: String },
    Hovering(bool),
}

/// The arguments ARE the widget's fields, one per WIT record member, and
/// phase 4 added two more (`text` and `box`). Grouping them into a struct
/// would mean a second shape beside the WIT record that has to be kept in
/// step with it by hand, which is the drift K-403 exists to prevent. The
/// sibling validator above carries the same allow for the same reason.
#[allow(clippy::too_many_arguments)]
pub(crate) fn widget_node_from_parts(
    raw_id: u64,
    raw_parent: Option<u64>,
    kind: WidgetKind,
    label: Option<String>,
    role: Option<String>,
    style: WidgetStyle,
    checked: Option<bool>,
    value: Option<f32>,
    selected: Option<u32>,
    text_cursor: Option<(u32, u32)>,
) -> Result<WidgetNode, ui::types::UiError> {
    // The rules themselves are shared with the browser Player
    // (krate-lowering, IC-956); only the error's WIT shape is this host's.
    krate_lowering::node_from_parts(
        raw_id,
        raw_parent,
        kind,
        label,
        role,
        style,
        checked,
        value,
        selected,
        text_cursor,
    )
    .map_err(|err| match err {
        krate_lowering::NodeError::InvalidWidget => ui::types::UiError::InvalidWidget,
        krate_lowering::NodeError::Unsupported(words) => ui::types::UiError::Unsupported(words),
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
        self.follow_widget_look();
        let size = match WindowSize::new(size.width, size.height) {
            Ok(size) => size,
            Err(err) => return Ok(Err(ui::types::UiError::Platform(err.to_string()))),
        };
        // `KRATE_SCREEN=390x844` (headless only): the window is that screen
        // whatever the app asked for, as on a phone, so a shot shows what a
        // phone shows. The app's own ask is still what `requested` records.
        let asked = size;
        let size = match (self.headless, std::env::var("KRATE_SCREEN").ok()) {
            (true, Some(screen)) => screen
                .split_once('x')
                .and_then(|(w, h)| {
                    Some((w.trim().parse::<u32>().ok()?, h.trim().parse::<u32>().ok()?))
                })
                .and_then(|(w, h)| WindowSize::new(w, h).ok())
                .unwrap_or(size),
            _ => size,
        };
        let options = match WindowOptions::new(title, size) {
            Ok(options) => options,
            Err(err) => return Ok(Err(ui::types::UiError::Platform(err.to_string()))),
        };

        // Build the font stack before the app can draw anything.
        //
        // Font discovery plus the first shaping pass costs about 58ms, and it
        // lands on whichever text call happens first -- so the FIRST FRAME of
        // any app that draws text overran a 16.6ms budget by 3.5x, and an app
        // whose first frame is its only frame (a --shoot, a splash, a
        // quick-run check) paid all of it inside the thing being measured.
        // Doing it here moves it off the frame path entirely (K-414).
        krate_adapter_common::vector_text::warm_text_engine();

        let title_note = options.title.clone();
        let requested = (asked.width as f32, asked.height as f32);
        match self.dispatcher().create_window(options) {
            Ok(id) => {
                self.windows.push(id);
                self.requested_sizes.borrow_mut().insert(id, requested);
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

    fn set_full_bleed(
        &mut self,
        window: u64,
        enabled: bool,
    ) -> wasmtime::Result<Result<(), ui::types::UiError>> {
        let id = match self.window_id(window) {
            Ok(id) => id,
            Err(err) => return Ok(Err(err)),
        };
        Ok(self
            .dispatcher()
            .set_full_bleed(id, enabled)
            .map_err(dispatch_error_to_ui_error))
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

impl Phase3GuiHost {
    /// Set a window's root from an already-converted node.
    ///
    /// Split out so Phase 4 can reach it. Phase 4's widget-kind has a case
    /// Phase 3's does not, so a Phase 4 node cannot be routed through Phase
    /// 3's `widget-node` without losing it -- but both phases converge on the
    /// host's own `WidgetNode`, which has the case. Everything after the
    /// conversion is therefore shared, and only the conversion is written
    /// twice (K-403).
    pub(crate) fn set_root_widget(
        &mut self,
        window: u64,
        root: WidgetNode,
    ) -> Result<(), ui::types::UiError> {
        let id = self.window_id(window)?;
        if let Err(err) = self.dispatcher().set_root(id, root) {
            return Err(dispatch_error_to_ui_error(err));
        }
        self.sync_native_widgets(id)
            .map_err(dispatch_error_to_ui_error)
    }

    /// Ask the person where to save, and mint a token for it.
    ///
    /// Lives here rather than in the Phase 4 host because the guard and the
    /// chosen-file store are this type's, and a second copy of the permission
    /// check is a second place for it to be wrong. Phase 4 calls this and
    /// converts the result into its own types (K-393).
    ///
    /// `Ok(None)` is a cancelled dialog: a normal answer, not a failure.
    /// Nothing is created -- the token records WHERE to write, and the writing
    /// happens when the app opens it, so asking and changing your mind leaves
    /// no empty file behind.
    pub(crate) fn ask_where_to_save(
        &mut self,
        title: &str,
        suggested: &str,
        filter: &str,
    ) -> Result<Option<(String, String)>, ui::types::UiError> {
        if self
            .runtime
            .guard()
            .check(&UapiCall::Ui(UiCall::Dialog {
                resource: crate::uapi::UiDialogResource::FileSave,
            }))
            .is_err()
        {
            return Err(ui::types::UiError::PermissionDenied);
        }
        // Headless -- --shoot, check-app, CI -- answers as cancelled, the rule
        // `open_file` already follows: no dialog exists for anyone to click,
        // and cancelling is an outcome every app handles.
        if self.headless {
            return Ok(None);
        }
        // A phone: the app writes into a file of Krate's, and once it lets
        // go the file goes to the system's save sheet (iOS) or its "save
        // to" picker (Android), where the person picks the place (K-1032).
        // The person's tap on Save in the app is the ask; the sheet is the
        // answer.
        #[cfg(any(target_os = "ios", target_os = "android"))]
        {
            let _ = title;
            let chosen = export_path(suggested, filter);
            let name = crate::chosen_files::ChosenFiles::display_name(&chosen);
            let Some(token) = self.chosen_files.borrow_mut().remember_export(chosen) else {
                return Err(ui::types::UiError::Unsupported(
                    "too many files chosen in one run".to_string(),
                ));
            };
            Ok(Some((name, token)))
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (title, suggested, filter);
            Ok(None)
        }
        #[cfg(not(any(target_os = "android", target_os = "ios", target_arch = "wasm32")))]
        {
            let chosen = match save_file_on_host(title, suggested, filter) {
                Ok(Some(path)) => path,
                Ok(None) => return Ok(None),
                Err(err) => return Err(ui::types::UiError::Unsupported(err)),
            };
            let name = crate::chosen_files::ChosenFiles::display_name(&chosen);
            let Some(token) = self.chosen_files.borrow_mut().remember(chosen) else {
                return Err(ui::types::UiError::Unsupported(
                    "too many files chosen in one run".to_string(),
                ));
            };
            Ok(Some((name, token)))
        }
    }

    /// Insert or update a node from an already-converted node. See
    /// [`Self::set_root_widget`] for why this is split out.
    pub(crate) fn upsert_widget(
        &mut self,
        window: u64,
        node: WidgetNode,
    ) -> Result<(), ui::types::UiError> {
        let id = self.window_id(window)?;
        if let Err(err) = self.dispatcher().upsert_node(id, node) {
            if let UiDispatchError::Adapter(adapter) = &err {
                if let Some(hint) = tree_mistake_hint(adapter) {
                    eprintln!("krate: {hint}");
                }
            }
            return Err(dispatch_error_to_ui_error(err));
        }
        self.sync_native_widgets(id)
            .map_err(dispatch_error_to_ui_error)
    }
}

/// What went wrong building a widget tree, in words (K-392).
///
/// The guest receives the error, but almost every app answers it with a bare
/// `return 1`, and the only tool that runs the app then reports "exit 1" with
/// nothing on stdout -- which of the setup calls failed was unrecoverable. The
/// host knows exactly which shape was wrong, so it says so on stderr, where
/// `check-app` and a developer's terminal both show it.
fn tree_mistake_hint(err: &UiAdapterError) -> Option<String> {
    Some(match err {
        UiAdapterError::MissingWidgetTree { .. } => {
            "a widget was added before the window had a root -- add the root first: \
             a node with no parent, or tree::set_root"
                .to_string()
        }
        UiAdapterError::MissingWidgetParent { id, parent: 0 } => format!(
            "widget {id} has no parent, but this window already has a root -- give it a \
             parent (usually the root's id)"
        ),
        UiAdapterError::MissingWidgetParent { id, parent } => format!(
            "widget {id} names parent {parent}, which is not in the tree yet -- add a \
             parent before its children"
        ),
        UiAdapterError::WidgetParentCycle { id } => {
            format!("widget {id} would be its own ancestor -- check its parent id")
        }
        UiAdapterError::DuplicateWidget { id } => {
            format!("widget {id} is the window's root and cannot be given a parent")
        }
        _ => return None,
    })
}

impl ui::tree::Host for Phase3GuiHost {
    fn set_root(
        &mut self,
        window: u64,
        root: ui::types::WidgetNode,
    ) -> wasmtime::Result<Result<(), ui::types::UiError>> {
        let root = match widget_node_from_wit(root) {
            Ok(root) => root,
            Err(err) => return Ok(Err(err)),
        };
        Ok(self.set_root_widget(window, root))
    }

    fn upsert_node(
        &mut self,
        window: u64,
        node: ui::types::WidgetNode,
    ) -> wasmtime::Result<Result<(), ui::types::UiError>> {
        let node = match widget_node_from_wit(node) {
            Ok(node) => node,
            Err(err) => return Ok(Err(err)),
        };
        Ok(self.upsert_widget(window, node))
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
        self.follow_widget_look();
        self.pump_native_windows();
        // A game never calls `wait` -- it polls at the top of every frame and
        // draws regardless. Driving only from `wait` means such an app is never
        // stepped and never released, so the run hangs until the outer
        // watchdog kills it and the stage blames the app for a defect it does
        // not have. Stepping here too is what makes the driver work for a
        // frame loop as well as an event loop.
        if self.usability.is_some() {
            if let Some(event) = self.drive_usability_step(false) {
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
        // A headless run ends on the wall-clock budget here as well as in
        // `wait`. `wait` was the only place that asked, which quietly exempted
        // every game: a frame loop drives itself with `poll` and `key-held` and
        // never waits, so the budget it was supposed to be held to was never
        // consulted and the run went on forever. A finished, correct game shot
        // its PNG in twenty seconds and then hung until an outer watchdog
        // killed it, which reads as a broken app rather than a runtime that
        // forgot to ask the question on this path.
        if let Some(close) = self.headless_budget_close_request() {
            return Ok(Some(close));
        }
        let event = self
            .poll_one_event()
            .map_err(|err| wasmtime::Error::msg(err.to_string()))?;
        if self.close_ignored_by_guest() {
            // The person clicked close twice and the app kept looping. The
            // doc on close_ignored_by_guest promises the runtime honours the
            // request on their behalf; swallowing the event (the old
            // behaviour) left a window nothing could shut, and a machine was
            // power-cycled over it (K-121). Same two-press contract as
            // Ctrl-C, ended the same way.
            return Err(crate::end_unanswered_close());
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
        {
            let mut asked = self.keys_asked.borrow_mut();
            if asked.len() < 32 && !asked.contains(&key) {
                asked.push(key.clone());
            }
        }
        self.update_pad();
        Ok(self.held_keys.borrow().contains(&key) || self.pad_held.borrow().contains(&key))
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
        self.follow_widget_look();
        // The app has built its window and is now waiting for input: the first
        // stable, fully-drawn frame. Capture here, before any budget check
        // that might end the run.
        self.maybe_take_screenshot();
        if self.first_wait_ms.get().is_none() {
            self.first_wait_ms
                .set(Some(self.born.elapsed().as_secs_f32() * 1000.0));
        }

        if let Some(event) = self.drive_scroll_step() {
            return Ok(Some(event));
        }

        let deadline = timeout_millis
            .map(|ms| std::time::Instant::now() + std::time::Duration::from_millis(u64::from(ms)));

        // A driven run is scripted, so it owns the budget question outright:
        // the driver decides when this run has seen enough and ends it itself.
        // The ordinary wall-clock budget below would cut the run off at five
        // seconds, which is before the ten-second self-close the driver exists
        // to catch.
        if self.usability.is_some() {
            if let Some(event) = self.drive_usability_step(true) {
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
                if self.close_ignored_by_guest() {
                    // Second unanswered close click; see poll (K-121).
                    return Err(crate::end_unanswered_close());
                }
                self.idle_waits.set(0);
                if std::env::var_os("KRATE_EVENT_TRACE").is_some() {
                    match event.as_ref() {
                        Some(ui::types::Event::Pointer(p)) => eprintln!(
                            "krate-event: pointer widget={:?} pressed={} at ({:.1},{:.1})",
                            p.widget, p.pressed, p.x, p.y
                        ),
                        other => eprintln!("krate-event: {:?}", other.map(event_name)),
                    }
                }
                return Ok(event);
            }
            if let Some(deadline) = deadline {
                if std::time::Instant::now() >= deadline {
                    // The guest asked for a timeout and it has arrived. Hand
                    // control back exactly as asked, so it stays free to treat
                    // the quiet round however it likes.
                    //
                    // A headless run does NOT need a counter here: the
                    // wall-clock budget above is checked before polling for
                    // exactly this reason -- an app that calls request-redraw
                    // every round always has an event waiting and would never
                    // look idle. Wall-clock is the only measure a self-feeding
                    // loop cannot outrun.
                    return Ok(None);
                }
            } else if self.usability.is_some() {
                // A driven run on an unbounded wait: the driver is paced by
                // wall clock and only advances when the app calls in, but an
                // app blocked in `wait(None)` with an empty queue never calls
                // in again -- so the script stalled here until the harness
                // killed it at 60 seconds, and a correctly written app
                // (wait(None) is exactly what the pack teaches) failed the
                // gate (K-094). Stepping the driver from inside the wait is
                // what closes that loop: it delivers the script's clicks and
                // resizes as ordinary events, which is how a person's input
                // would arrive anyway.
                if let Some(event) = self.drive_usability_step(true) {
                    self.idle_waits.set(0);
                    return Ok(Some(event));
                }
            } else if let Some(close) = self.headless_close_request() {
                // An unbounded wait on a headless host can never return on its
                // own: nothing exists that could deliver an event. Reporting
                // the close is the only honest way out, and every GUI app
                // already handles it, so the app shuts down through its normal
                // path and still saves on the way out.
                return Ok(Some(close));
            }
            // An adapter that can park inside its native event loop wakes
            // the instant input arrives, so its park can be LONG -- waking
            // is event-driven, and short slices just burn battery (a phone
            // reached thermal-serious from exactly that churn). The blind
            // sleep stays short because nothing can interrupt it.
            //
            // Clamped to the wait's own deadline: a 10ms slice against a
            // 16ms timeout used to overshoot to ~20ms, and a game pacing
            // itself with wait(16) ran at 41fps on an idle machine -- the
            // slice must never be the thing that misses the frame.
            let slice = std::time::Duration::from_millis(match deadline {
                Some(_) => WAIT_POLL_INTERVAL_MILLIS,
                None => 250,
            });
            let park = match deadline {
                Some(deadline) => {
                    slice.min(deadline.saturating_duration_since(std::time::Instant::now()))
                }
                None => slice,
            };
            if park.is_zero() {
                // The deadline has arrived. Spinning back to the top without
                // yielding is what pins a core: an app that falls behind its
                // own frame budget asks for wait(1ms), the deadline is
                // already past by the time we get here, and this loop then
                // runs pump+poll flat out until the next check. Aurora does
                // exactly that -- it paces to 60fps, misses, and the runtime
                // burns 100% of a core while the animation visibly stutters
                // (K-194).
                //
                // yield_now() is not enough and was measured: with a spare
                // core the scheduler hands the thread straight back and the
                // CPU stays pinned at 100%. A real sleep is what caps the
                // spin. One millisecond is short enough that a late frame
                // still catches up promptly and long enough that the loop
                // stops being a busy-wait.
                std::thread::sleep(std::time::Duration::from_millis(1));
                continue;
            }
            if !self.dispatcher().park_for_events(park) {
                std::thread::sleep(park);
            }
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
        // Attribution is the APP's name, fixed at consent, never the guest's
        // per-call title (IC-271, see `attribution`).
        let sender = self.attribution();
        Ok(
            crate::desktop_host::notify(&title, &body, &sender, granted).map_err(|err| match err {
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
        window: u64,
        title: String,
        filter: String,
    ) -> wasmtime::Result<Result<Option<ui::dialog::ChosenFile>, ui::types::UiError>> {
        // Checked before anything else happens or is revealed -- the dialog
        // host functions ran completely unguarded until K-086, so any app
        // could pop pickers its manifest never asked for.
        if self
            .runtime
            .guard()
            .check(&UapiCall::Ui(UiCall::Dialog {
                resource: crate::uapi::UiDialogResource::FileOpen,
            }))
            .is_err()
        {
            return Ok(Err(ui::types::UiError::PermissionDenied));
        }
        // Headless runs -- --shoot, check-app, CI -- must never block on a
        // native dialog nobody can click. A cancelled dialog is the honest
        // answer there, and it is a normal outcome the app already handles.
        if self.headless {
            return Ok(Ok(None));
        }
        // Phones have no rfd backend: the adapter shows the platform's own
        // picker (iOS: the Files picker; Android: its document picker).
        // Every dialog there used to answer as cancelled, so a Studio-made
        // recipe book's "Add photo" did nothing on an iPhone (IC-915). A
        // host without a picker still answers cancelled.
        #[cfg(any(target_os = "android", target_os = "ios", target_arch = "wasm32"))]
        {
            let _ = &title;
            let Ok(id) = self.window_id(window) else {
                return Ok(Ok(None));
            };
            let Some(chosen) = self.dispatcher().choose_path(id, false, &filter) else {
                return Ok(Ok(None));
            };
            let name = crate::chosen_files::ChosenFiles::display_name(&chosen);
            let Some(token) = self.chosen_files.borrow_mut().remember(chosen) else {
                return Ok(Err(ui::types::UiError::Unsupported(
                    "too many files chosen in one run".to_string(),
                )));
            };
            Ok(Ok(Some(ui::dialog::ChosenFile { name, token })))
        }
        #[cfg(not(any(target_os = "android", target_os = "ios", target_arch = "wasm32")))]
        {
            let _ = window;
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
    }

    /// Show the system's folder picker; the pick is the grant.
    ///
    /// The app receives a name and a token. Files inside the picked folder
    /// are reached through `picked/<token>/...` paths on the ordinary fs
    /// calls -- the resolver mounts the token for this run and containment
    /// is enforced against the picked folder exactly as it is against the
    /// sandbox. Nothing about the folder's location reaches the app, and
    /// the grant dies with the run.
    fn open_folder(
        &mut self,
        window: u64,
        title: String,
    ) -> wasmtime::Result<Result<Option<ui::dialog::ChosenFolder>, ui::types::UiError>> {
        if self
            .runtime
            .guard()
            .check(&UapiCall::Ui(UiCall::Dialog {
                resource: crate::uapi::UiDialogResource::OpenFolder,
            }))
            .is_err()
        {
            return Ok(Err(ui::types::UiError::PermissionDenied));
        }
        if self.headless {
            return Ok(Ok(None));
        }
        // Phones: the platform's own folder picker, through the adapter (see
        // open_file). A music player made in Studio could not be given a
        // folder of songs on an iPhone before this.
        #[cfg(any(target_os = "android", target_os = "ios", target_arch = "wasm32"))]
        {
            let _ = &title;
            let Ok(id) = self.window_id(window) else {
                return Ok(Ok(None));
            };
            let Some(folder) = self.dispatcher().choose_path(id, true, "") else {
                return Ok(Ok(None));
            };
            let name = crate::chosen_files::ChosenFiles::display_name(&folder);
            let Some(token) = self.chosen_files.borrow_mut().remember_folder(folder) else {
                return Ok(Err(ui::types::UiError::Unsupported(
                    "too many files or folders chosen in one run".to_string(),
                )));
            };
            Ok(Ok(Some(ui::dialog::ChosenFolder { name, token })))
        }
        #[cfg(not(any(target_os = "android", target_os = "ios", target_arch = "wasm32")))]
        {
            let _ = window;
            let mut dialog = rfd::FileDialog::new();
            if !title.is_empty() {
                dialog = dialog.set_title(title);
            }
            let Some(folder) = dialog.pick_folder() else {
                return Ok(Ok(None));
            };
            let name = crate::chosen_files::ChosenFiles::display_name(&folder);
            let Some(token) = self.chosen_files.borrow_mut().remember_folder(folder) else {
                return Ok(Err(ui::types::UiError::Unsupported(
                    "too many files or folders chosen in one run".to_string(),
                )));
            };
            Ok(Ok(Some(ui::dialog::ChosenFolder { name, token })))
        }
    }

    /// Show a message and wait for the person to dismiss it.
    fn message(
        &mut self,
        _window: u64,
        title: String,
        body: String,
    ) -> wasmtime::Result<Result<(), ui::types::UiError>> {
        if self
            .runtime
            .guard()
            .check(&UapiCall::Ui(UiCall::Dialog {
                resource: crate::uapi::UiDialogResource::Message,
            }))
            .is_err()
        {
            return Ok(Err(ui::types::UiError::PermissionDenied));
        }
        // Headless: nobody is there to dismiss it, and rfd would block the
        // run forever. Shown-and-dismissed is the honest no-op.
        if self.headless {
            return Ok(Ok(()));
        }
        // Android: shown-and-dismissed, the headless rule, until M3.
        #[cfg(any(target_os = "android", target_os = "ios", target_arch = "wasm32"))]
        {
            let _ = (&title, &body);
            Ok(Ok(()))
        }
        #[cfg(not(any(target_os = "android", target_os = "ios", target_arch = "wasm32")))]
        {
            rfd::MessageDialog::new()
                .set_title(&title)
                .set_description(&body)
                .set_buttons(rfd::MessageButtons::Ok)
                .show();
            Ok(Ok(()))
        }
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
        if self
            .runtime
            .guard()
            .check(&UapiCall::Ui(UiCall::Dialog {
                resource: crate::uapi::UiDialogResource::Confirm,
            }))
            .is_err()
        {
            return Ok(Err(ui::types::UiError::PermissionDenied));
        }
        // Headless: a dismissed dialog counts as "no" by this function's own
        // rule, and nobody is there to answer -- so the answer is no.
        if self.headless {
            return Ok(Ok(false));
        }
        // Android: dismissed means no, the function's own rule, until M3.
        #[cfg(any(target_os = "android", target_os = "ios", target_arch = "wasm32"))]
        {
            let _ = (&title, &body);
            Ok(Ok(false))
        }
        #[cfg(not(any(target_os = "android", target_os = "ios", target_arch = "wasm32")))]
        {
            let answer = rfd::MessageDialog::new()
                .set_title(&title)
                .set_description(&body)
                .set_buttons(rfd::MessageButtons::YesNo)
                .show();
            Ok(Ok(answer == rfd::MessageDialogResult::Yes))
        }
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

/// A check step's chord: Shift, and Cmd on a Mac or Ctrl elsewhere.
fn chord_modifiers(shift: bool, primary: bool) -> krate_adapter_common::ui::Modifiers {
    let mac = cfg!(target_os = "macos");
    krate_adapter_common::ui::Modifiers {
        shift,
        control: primary && !mac,
        alt: false,
        meta: primary && mac,
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

/// Map the WIT text style onto the raster's, field for field.
fn canvas_style(
    style: &gfx::types::TextStyle,
) -> krate_adapter_common::vector_text::CanvasTextStyle {
    use krate_adapter_common::vector_text::{CanvasFontFamily, CanvasTextStyle};
    CanvasTextStyle {
        weight: style.weight,
        italic: style.italic,
        letter_spacing: style.letter_spacing,
        family: match style.family {
            gfx::types::FontFamily::Sans => CanvasFontFamily::Sans,
            gfx::types::FontFamily::Serif => CanvasFontFamily::Serif,
            gfx::types::FontFamily::Mono => CanvasFontFamily::Mono,
        },
    }
}

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
        // Raster at the display's density, keep the app in logical units
        // (K-088): the placement blit is then 1:1 instead of an upscale.
        let scale = self.canvas_backing_scale(window_id);
        let surface = match CanvasSurface::new_scaled(
            rect.0.max(1.0) as u32,
            rect.1.max(1.0) as u32,
            scale,
        ) {
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

    fn set_design_size(
        &mut self,
        canvas: u64,
        size: gfx::types::Size,
    ) -> wasmtime::Result<Result<(), gfx::types::GfxError>> {
        let mut canvases = self.canvases.borrow_mut();
        let Some((_, _, surface)) = canvases.get_mut(&canvas) else {
            return Ok(Err(gfx::types::GfxError::InvalidTarget));
        };
        surface.set_design_size(size.width, size.height);
        // The app's own design replaces any the host chose for it.
        self.implicit_designs.borrow_mut().remove(&canvas);
        Ok(Ok(()))
    }

    fn canvas_size(
        &mut self,
        canvas: u64,
    ) -> wasmtime::Result<Result<gfx::types::Size, gfx::types::GfxError>> {
        // An app that asks lays itself out from the answer: it is responsive,
        // and a design the host chose for it (see `fit_unasked_canvas`) goes.
        self.canvases_that_asked.borrow_mut().insert(canvas);
        if self.implicit_designs.borrow_mut().remove(&canvas) {
            if let Some((_, _, surface)) = self.canvases.borrow_mut().get_mut(&canvas) {
                surface.set_design_size(0.0, 0.0);
            }
        }
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
        // An app that fixed its coordinate system asks about THAT system,
        // not the window: it draws in design space and the host maps it.
        if let Some((dw, dh)) = surface.design_size() {
            return Ok(Ok(gfx::types::Size {
                width: dw,
                height: dh,
            }));
        }
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
        self.record_op(
            canvas,
            crate::canvas_list::CanvasOp::Clear(pack_color(fill.r, fill.g, fill.b, fill.a)),
        );
        if self.lists_enabled() {
            return Ok(Ok(()));
        }
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
        self.record_op(
            canvas,
            crate::canvas_list::CanvasOp::SetClip(Some((x, y, w, h))),
        );
        if self.lists_enabled() {
            return Ok(Ok(()));
        }
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
        self.record_op(canvas, crate::canvas_list::CanvasOp::SetClip(None));
        if self.lists_enabled() {
            return Ok(Ok(()));
        }
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
        self.record_op(
            canvas,
            crate::canvas_list::CanvasOp::FillRect {
                rect: (area.x, area.y, area.width, area.height),
                color: pack_color(fill.r, fill.g, fill.b, fill.a),
            },
        );
        if self.lists_enabled() {
            return Ok(Ok(()));
        }
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
        self.record_op(
            canvas,
            crate::canvas_list::CanvasOp::FillCircle {
                center: (center.x, center.y),
                radius,
                color: pack_color(fill.r, fill.g, fill.b, fill.a),
            },
        );
        if self.lists_enabled() {
            return Ok(Ok(()));
        }
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
        self.record_op(
            canvas,
            crate::canvas_list::CanvasOp::StrokeCircle {
                center: (center.x, center.y),
                radius,
                width,
                color: pack_color(stroke.r, stroke.g, stroke.b, stroke.a),
            },
        );
        if self.lists_enabled() {
            return Ok(Ok(()));
        }
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
        self.record_op(
            canvas,
            crate::canvas_list::CanvasOp::FillRoundRect {
                rect: (area.x, area.y, area.width, area.height),
                radii: (
                    radii.top_left,
                    radii.top_right,
                    radii.bottom_right,
                    radii.bottom_left,
                ),
                color: pack_color(fill.r, fill.g, fill.b, fill.a),
            },
        );
        if self.lists_enabled() {
            return Ok(Ok(()));
        }
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
        self.record_op(
            canvas,
            crate::canvas_list::CanvasOp::StrokeRoundRect {
                rect: (area.x, area.y, area.width, area.height),
                radii: (
                    radii.top_left,
                    radii.top_right,
                    radii.bottom_right,
                    radii.bottom_left,
                ),
                width,
                color: pack_color(stroke.r, stroke.g, stroke.b, stroke.a),
            },
        );
        if self.lists_enabled() {
            return Ok(Ok(()));
        }
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
        self.record_op(
            canvas,
            crate::canvas_list::CanvasOp::DropShadowRoundRect {
                rect: (area.x, area.y, area.width, area.height),
                radii: (
                    radii.top_left,
                    radii.top_right,
                    radii.bottom_right,
                    radii.bottom_left,
                ),
                blur,
                color: pack_color(shadow.r, shadow.g, shadow.b, shadow.a),
            },
        );
        if self.lists_enabled() {
            return Ok(Ok(()));
        }
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
        self.record_op(
            canvas,
            crate::canvas_list::CanvasOp::LinearGradientStops {
                rect: (area.x, area.y, area.width, area.height),
                angle_degrees,
                stops: stops
                    .iter()
                    .map(|s| {
                        (
                            s.offset,
                            pack_color(s.color.r, s.color.g, s.color.b, s.color.a),
                        )
                    })
                    .collect(),
            },
        );
        if self.lists_enabled() {
            return Ok(Ok(()));
        }
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
        self.record_op(
            canvas,
            crate::canvas_list::CanvasOp::RadialGradient {
                center: (center.x, center.y),
                radius,
                inner: pack_color(inner.r, inner.g, inner.b, inner.a),
                outer: pack_color(outer.r, outer.g, outer.b, outer.a),
            },
        );
        if self.lists_enabled() {
            return Ok(Ok(()));
        }
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
        self.record_op(
            canvas,
            crate::canvas_list::CanvasOp::LinearGradient {
                rect: (area.x, area.y, area.width, area.height),
                top: pack_color(top.r, top.g, top.b, top.a),
                bottom: pack_color(bottom.r, bottom.g, bottom.b, bottom.a),
            },
        );
        if self.lists_enabled() {
            return Ok(Ok(()));
        }
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
        self.record_op(
            canvas,
            crate::canvas_list::CanvasOp::StrokeRect {
                rect: (area.x, area.y, area.width, area.height),
                width,
                color: pack_color(stroke.r, stroke.g, stroke.b, stroke.a),
            },
        );
        if self.lists_enabled() {
            return Ok(Ok(()));
        }
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
        self.record_op(
            canvas,
            crate::canvas_list::CanvasOp::Text {
                origin: (origin.x, origin.y),
                font_size,
                color: pack_color(ink.r, ink.g, ink.b, ink.a),
                weight: 400,
                italic: false,
                letter_spacing: 0.0,
                family: 0,
                text: text.clone(),
            },
        );
        if self.lists_enabled() {
            return Ok(Ok(()));
        }
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

    fn draw_text_styled(
        &mut self,
        canvas: u64,
        text: String,
        origin: gfx::types::Point,
        font_size: f32,
        ink: gfx::types::Color,
        style: gfx::types::TextStyle,
    ) -> wasmtime::Result<Result<(), gfx::types::GfxError>> {
        self.record_op(
            canvas,
            crate::canvas_list::CanvasOp::Text {
                origin: (origin.x, origin.y),
                font_size,
                color: pack_color(ink.r, ink.g, ink.b, ink.a),
                weight: style.weight,
                italic: style.italic,
                letter_spacing: style.letter_spacing,
                family: match style.family {
                    gfx::types::FontFamily::Sans => 0,
                    gfx::types::FontFamily::Serif => 1,
                    gfx::types::FontFamily::Mono => 2,
                },
                text: text.clone(),
            },
        );
        if self.lists_enabled() {
            return Ok(Ok(()));
        }
        let mut canvases = self.canvases.borrow_mut();
        let Some((_, _, surface)) = canvases.get_mut(&canvas) else {
            return Ok(Err(gfx::types::GfxError::InvalidTarget));
        };
        surface.text_styled(
            &text,
            origin.x,
            origin.y,
            font_size,
            pack_color(ink.r, ink.g, ink.b, ink.a),
            canvas_style(&style),
        );
        Ok(Ok(()))
    }

    fn measure_text_styled(
        &mut self,
        canvas: u64,
        text: String,
        font_size: f32,
        style: gfx::types::TextStyle,
    ) -> wasmtime::Result<Result<gfx::types::TextMetrics, gfx::types::GfxError>> {
        let canvases = self.canvases.borrow();
        let Some((_, _, surface)) = canvases.get(&canvas) else {
            return Ok(Err(gfx::types::GfxError::InvalidTarget));
        };
        let m = surface.measure_text_styled(&text, font_size, canvas_style(&style));
        Ok(Ok(gfx::types::TextMetrics {
            width: m.width,
            height: m.height,
            ascent: m.ascent,
            descent: m.descent,
        }))
    }

    fn stroke_arc(
        &mut self,
        canvas: u64,
        center: gfx::types::Point,
        radius: f32,
        start_degrees: f32,
        sweep_degrees: f32,
        width: f32,
        stroke: gfx::types::Color,
    ) -> wasmtime::Result<Result<(), gfx::types::GfxError>> {
        self.record_op(
            canvas,
            crate::canvas_list::CanvasOp::StrokeArc {
                center: (center.x, center.y),
                radius,
                start_degrees,
                sweep_degrees,
                width,
                color: pack_color(stroke.r, stroke.g, stroke.b, stroke.a),
            },
        );
        if self.lists_enabled() {
            return Ok(Ok(()));
        }
        let mut canvases = self.canvases.borrow_mut();
        let Some((_, _, surface)) = canvases.get_mut(&canvas) else {
            return Ok(Err(gfx::types::GfxError::InvalidTarget));
        };
        surface.stroke_arc(
            center.x,
            center.y,
            radius,
            start_degrees,
            sweep_degrees,
            width,
            pack_color(stroke.r, stroke.g, stroke.b, stroke.a),
        );
        Ok(Ok(()))
    }

    fn draw_pixels_round(
        &mut self,
        canvas: u64,
        area: gfx::types::Rect,
        radii: gfx::types::CornerRadii,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    ) -> wasmtime::Result<Result<(), gfx::types::GfxError>> {
        self.record_op(
            canvas,
            crate::canvas_list::CanvasOp::PixelsRound {
                rect: (area.x, area.y, area.width, area.height),
                radii: (
                    radii.top_left,
                    radii.top_right,
                    radii.bottom_right,
                    radii.bottom_left,
                ),
                image: std::sync::Arc::new(krate_adapter_common::ui::ImagePixels {
                    width,
                    height,
                    rgba: rgba.clone(),
                }),
            },
        );
        if self.lists_enabled() {
            return Ok(Ok(()));
        }
        let image = match ImagePixels::new(width, height, rgba) {
            Ok(image) => image,
            Err(error) => return Ok(Err(gfx::types::GfxError::Unsupported(error.to_string()))),
        };
        let mut canvases = self.canvases.borrow_mut();
        let Some((_, _, surface)) = canvases.get_mut(&canvas) else {
            return Ok(Err(gfx::types::GfxError::InvalidTarget));
        };
        surface.draw_pixels_round(
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
            &image,
        );
        Ok(Ok(()))
    }

    fn draw_pixels(
        &mut self,
        canvas: u64,
        area: gfx::types::Rect,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    ) -> wasmtime::Result<Result<(), gfx::types::GfxError>> {
        // Only copy the picture when something will read the copy: a whole
        // frame is megabytes, and cloning it every frame for a list no
        // adapter consumes cost a full-frame copy for nothing.
        if self.lists_enabled() || self.inspect_layout.get() {
            self.record_op(
                canvas,
                crate::canvas_list::CanvasOp::Pixels {
                    rect: (area.x, area.y, area.width, area.height),
                    image: std::sync::Arc::new(krate_adapter_common::ui::ImagePixels {
                        width,
                        height,
                        rgba: rgba.clone(),
                    }),
                },
            );
        }
        if self.lists_enabled() {
            return Ok(Ok(()));
        }
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
        surface.draw_pixels_owned(area.x, area.y, area.width, area.height, image);
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
        self.record_op(
            canvas,
            crate::canvas_list::CanvasOp::Sprite {
                center: (center.x, center.y),
                dst: (dst.width, dst.height),
                angle,
                image: std::sync::Arc::new(krate_adapter_common::ui::ImagePixels {
                    width,
                    height,
                    rgba: rgba.clone(),
                }),
            },
        );
        if self.lists_enabled() {
            return Ok(Ok(()));
        }
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
        // The app's work for the event it was handed ends here (IC-915).
        if let Some(since) = self.guest_since.take() {
            self.frame_costs
                .borrow_mut()
                .push(since.elapsed().as_secs_f32() * 1000.0);
        }
        self.fit_unasked_canvas(canvas);
        // The pad is drawn with the app's design switched off, and the design
        // comes back only after the frame is published: the CPU raster fills
        // the bands around a design with its edge colours when it publishes,
        // which would paint over the pad there.
        let design = self.draw_pad(canvas);
        let result = self.present_frame(canvas);
        if let Some((dw, dh)) = design {
            if let Some((_, _, surface)) = self.canvases.borrow_mut().get_mut(&canvas) {
                surface.set_design_size(dw, dh);
            }
        }
        result
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
        let scene = match SceneBackend::new(rect.0.max(1.0) as u32, rect.1.max(1.0) as u32) {
            Ok(scene) => scene,
            Err(error) => return Ok(Err(gfx::types::GfxError::Unsupported(error.to_string()))),
        };
        // One line of truth per scene, the same promise the 2D presenter makes
        // about which silicon drew the frame. "The game lags" debugging starts
        // by knowing whether this ran on the GPU at all.
        eprintln!("krate: 3D scene renders on the {}", scene.backend().name());
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

    fn smooth(
        &mut self,
        scene: u64,
        vertices: Vec<f32>,
        normals: Vec<f32>,
        uvs: Vec<f32>,
        texture: u64,
        tint: gfx::types::Color,
    ) -> wasmtime::Result<Result<(), gfx::types::GfxError>> {
        let mut scenes = self.scenes.borrow_mut();
        let Some((_, _, surface)) = scenes.get_mut(&scene) else {
            return Ok(Err(gfx::types::GfxError::InvalidTarget));
        };
        surface.smooth(
            &vertices,
            &normals,
            &uvs,
            texture,
            (tint.r, tint.g, tint.b, tint.a),
        );
        Ok(Ok(()))
    }

    fn set_lighting(
        &mut self,
        scene: u64,
        lighting: gfx::scene3d::Lighting,
    ) -> wasmtime::Result<Result<(), gfx::types::GfxError>> {
        // Three floats each, or the direction describes nothing. Refused
        // rather than padded, the same way `camera` refuses a short vector.
        if !lighting.fill_direction.is_empty() && lighting.fill_direction.len() != 3 {
            return Ok(Err(gfx::types::GfxError::Unsupported(
                "a fill light takes three floats for its direction, or none at all".to_string(),
            )));
        }
        let mut scenes = self.scenes.borrow_mut();
        let Some((_, _, surface)) = scenes.get_mut(&scene) else {
            return Ok(Err(gfx::types::GfxError::InvalidTarget));
        };
        // An empty direction means no fill light, which is how an app turns
        // one off without a second call.
        let fill = if lighting.fill_direction.len() == 3 {
            Some((
                [
                    lighting.fill_direction[0],
                    lighting.fill_direction[1],
                    lighting.fill_direction[2],
                ],
                [
                    lighting.fill_color.r,
                    lighting.fill_color.g,
                    lighting.fill_color.b,
                ],
            ))
        } else {
            None
        };
        let value = krate_scene3d_gpu::Lighting {
            ambient_flat: lighting.ambient,
            ambient_smooth: lighting.ambient,
            specular: lighting.specular,
            shininess: lighting.shininess,
            fog_density: lighting.fog_density,
            fog_color: [
                lighting.fog_color.r,
                lighting.fog_color.g,
                lighting.fog_color.b,
            ],
            fill,
            shadow_radius: lighting.shadow_radius,
            shadow_softness: lighting.shadow_softness,
            exposure: lighting.exposure,
        };
        match surface.set_lighting(value) {
            Ok(()) => Ok(Ok(())),
            Err(error) => Ok(Err(gfx::types::GfxError::Unsupported(error.to_string()))),
        }
    }

    fn normal_mapped(
        &mut self,
        scene: u64,
        vertices: Vec<f32>,
        normals: Vec<f32>,
        tangents: Vec<f32>,
        uvs: Vec<f32>,
        texture: u64,
        normal_texture: u64,
        tint: gfx::types::Color,
    ) -> wasmtime::Result<Result<(), gfx::types::GfxError>> {
        let mut scenes = self.scenes.borrow_mut();
        let Some((_, _, surface)) = scenes.get_mut(&scene) else {
            return Ok(Err(gfx::types::GfxError::InvalidTarget));
        };
        surface.normal_mapped(
            &vertices,
            &normals,
            &tangents,
            &uvs,
            texture,
            normal_texture,
            (tint.r, tint.g, tint.b, tint.a),
        );
        Ok(Ok(()))
    }

    fn unlit(
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
        surface.unlit(&vertices, &uvs, texture, (tint.r, tint.g, tint.b, tint.a));
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
        // A scene game loop lives in present exactly like a canvas one; the
        // second unanswered close click ends the app here too (K-121).
        if self.close_ignored_by_guest() {
            return Err(crate::end_unanswered_close());
        }
        // Pace the frame, exactly as the 2D path does.
        //
        // This was missing entirely: a 3D game loop calls present with no
        // sleep of its own, so it free-ran at whatever the rasterizer could
        // manage -- measured at roughly three times the display's rate --
        // burning a core to draw frames no panel would ever show. A 2D canvas
        // has paced since it shipped; a scene never did, and the difference
        // was invisible because both look fine in a screenshot.
        self.pace_frame();
        let (window, widget, image) = {
            // Mutable because presenting is what fills the frame: triangles
            // are queued as they are drawn and rasterized here, once, across
            // every core.
            let mut scenes = self.scenes.borrow_mut();
            let Some((window, widget, surface)) = scenes.get_mut(&scene) else {
                return Ok(Err(gfx::types::GfxError::InvalidTarget));
            };
            // Render, publish the GPU texture, and read the frame back only
            // when something still needs CPU pixels.
            //
            // The readback is the single largest cost in the 3D path -- 4,346us
            // at p50 on a 1600x900 scene, 26% of a frame budget (K-405) -- and
            // it exists only because a texture cannot cross wgpu devices. Now
            // that the scene and the window presenter share one device, a host
            // that can blit the texture never needs the pixels.
            //
            // ALWAYS false for now: the direct path is disabled (K-407).
            //
            // The presenter had to go back to its own wgpu device, because the
            // shared one is created without a display handle and cannot own a
            // window surface -- pairing them crashed wgpu outright. With no
            // presenter on the shared device, nothing consumes a published
            // texture, so publishing one and returning no pixels leaves the
            // canvas with nothing at all.
            //
            // That is exactly what shipped for one commit: a windowed run drew
            // a BLACK SCENE with the 2D overlay perfectly readable on top of
            // it. Headless `--shoot` was unaffected, because a pending
            // screenshot already forced this to false -- so every screenshot
            // taken to verify the work looked right while the thing a person
            // actually opens was broken.
            //
            // Re-enable this with the presenter, not on its own: it is only
            // correct when something on the shared device is there to blit.
            let direct = false;
            let image = match surface.render_and_publish(widget.get(), direct) {
                Ok(image) => image,
                Err(error) => return Ok(Err(gfx::types::GfxError::Platform(error.to_string()))),
            };
            (*window, *widget, image)
        };
        if let Some(image) = image {
            self.images
                .borrow_mut()
                .insert((window, widget), std::sync::Arc::new(image));
        }
        let result = self
            .sync_native_widgets(window)
            .map_err(|error| gfx::types::GfxError::Platform(error.to_string()));
        // A presented scene is a real drawn 3D frame -- capture here so a game
        // that steers with key-held and never calls wait still gets shot with
        // its scene on screen, not the blank frame before the first present.
        if result.is_ok() {
            self.maybe_take_screenshot();
        }
        // The measured frame is done with; the next draw op begins another.
        self.frame_ended.set(true);
        self.end_stepped_frame();
        Ok(result)
    }
}

/// One frame at 60 Hz. Both present paths wait out the remainder of this, so
/// a guest loop that never sleeps does not spin a core.
const FRAME_BUDGET: std::time::Duration = std::time::Duration::from_micros(16_667);

/// Version of the sidecar a shot carries.
pub const SHOT_SIDECAR_SCHEMA: &str = "krate.shot.v1";
/// The one renderer a headless shot goes through: the shared CPU painter
/// every native host lowers to. Named so a golden made by it is never
/// compared with a frame from something else.
pub const SHARED_PAINTER: &str = "krate-shared-painter";
/// How the bytes in the PNG are to be read.
pub const SHOT_COLOR_SPACE: &str = "sRGB 8-bit RGBA, straight alpha, from an ARGB8888 framebuffer";

/// What painted a shot, written beside it as `<shot>.json`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ShotSidecar {
    pub schema: String,
    pub renderer: String,
    pub scale: f32,
    pub width: u32,
    pub height: u32,
    pub logical_width: u32,
    pub logical_height: u32,
    pub color_space: String,
    pub os: String,
    pub arch: String,
    pub runtime: String,
}

/// The sidecar's path for a shot: the PNG's path with `.json` appended,
/// so `frame.png` and `frame.png.json` sit together.
pub fn shot_sidecar_path(shot: &std::path::Path) -> std::path::PathBuf {
    let mut name = shot.as_os_str().to_os_string();
    name.push(".json");
    std::path::PathBuf::from(name)
}

fn write_shot_sidecar(shot: &std::path::Path, sidecar: &ShotSidecar) -> std::io::Result<()> {
    let json = serde_json::to_string_pretty(sidecar).map_err(std::io::Error::other)?;
    std::fs::write(shot_sidecar_path(shot), json + "\n")
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

impl Phase3GuiHost {
    /// Whether the camera grant is missing for this run.
    ///
    /// Checked on every call rather than once at open, so a policy that
    /// changes mid-run takes effect on the next frame read rather than at the
    /// next launch.
    fn camera_denied(&self) -> bool {
        self.runtime
            .guard()
            .check(&UapiCall::Camera(CameraCall::Capture))
            .is_err()
    }

    /// Whether the person granted `net.connect` to the host an address
    /// names: the judgment a fetch gets, for a video streamed from it.
    pub(crate) fn may_reach(&self, address: &str) -> bool {
        let Ok(endpoint) = krate_adapter_common::net::parse_url_endpoint(address) else {
            return false;
        };
        self.runtime
            .guard()
            .check(&UapiCall::Net(crate::uapi::NetCall::Connect {
                host: endpoint.host,
                port: endpoint.port,
            }))
            .is_ok()
    }
}

fn camera_config(config: camera::types::StreamConfig) -> CameraConfig {
    CameraConfig {
        width: config.width,
        height: config.height,
        fps: config.fps,
        format: match config.format {
            camera::types::FrameFormat::Rgba8 => CameraFrameFormat::Rgba8,
        },
    }
}

fn camera_error(error: CameraError) -> camera::types::CameraError {
    match error {
        CameraError::InvalidStream => camera::types::CameraError::InvalidStream,
        CameraError::DeviceUnavailable => camera::types::CameraError::DeviceUnavailable,
        // Kept separate from PermissionDenied all the way to the app: one is
        // Krate's wall, the other is the operating system's, and only the app
        // can tell the person which settings screen to open.
        CameraError::SystemDenied => camera::types::CameraError::SystemDenied,
        CameraError::InvalidConfig(message) => camera::types::CameraError::Unsupported(message),
        CameraError::Unsupported(message) => camera::types::CameraError::Unsupported(message),
        CameraError::Platform(message) => camera::types::CameraError::Platform(message),
    }
}

fn capture_error(error: CaptureError) -> audio::types::AudioError {
    match error {
        CaptureError::InvalidStream => audio::types::AudioError::InvalidStream,
        CaptureError::DeviceUnavailable => audio::types::AudioError::DeviceUnavailable,
        CaptureError::InvalidConfig(message) => audio::types::AudioError::Unsupported(message),
        CaptureError::Platform(message) => audio::types::AudioError::Platform(message),
        // The guest contract already has the right variant here, so unlike
        // store.sql this needs no translation into Io -- just the words.
        #[cfg(target_arch = "wasm32")]
        CaptureError::Unsupported => audio::types::AudioError::Unsupported(
            "listening through the microphone needs the app on your computer, \
             which a browser preview cannot do -- download it to use this part"
                .to_string(),
        ),
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
        #[cfg(target_arch = "wasm32")]
        PlaybackError::Unsupported => audio::types::AudioError::Unsupported(
            "playing sound needs the app on your computer, which a browser \
             preview cannot do -- download it to use this part"
                .to_string(),
        ),
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

/// How far ahead a playback stream is (K-1011). Phase 4 only; the same
/// permission as the stream it measures.
impl crate::phase4_gui_bindings::krate::audio::playback_queue::Host for Phase3GuiHost {
    fn queued(
        &mut self,
        stream_id: u64,
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
            .queued(stream_id)
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

impl camera::types::Host for Phase3GuiHost {}

impl camera::capture::Host for Phase3GuiHost {
    fn devices(
        &mut self,
    ) -> wasmtime::Result<Result<Vec<camera::types::DeviceInfo>, camera::types::CameraError>> {
        if self.camera_denied() {
            return Ok(Err(camera::types::CameraError::PermissionDenied));
        }
        Ok(self
            .cameras
            .devices()
            .map(|devices| {
                devices
                    .into_iter()
                    .map(|device| camera::types::DeviceInfo {
                        id: device.id,
                        label: device.label,
                    })
                    .collect()
            })
            .map_err(camera_error))
    }

    fn open(
        &mut self,
        device: String,
        config: camera::types::StreamConfig,
    ) -> wasmtime::Result<Result<u64, camera::types::CameraError>> {
        if self.camera_denied() {
            return Ok(Err(camera::types::CameraError::PermissionDenied));
        }
        Ok(self
            .cameras
            .open(&device, camera_config(config))
            .map_err(camera_error))
    }

    fn info(
        &mut self,
        stream_id: u64,
    ) -> wasmtime::Result<Result<camera::types::FrameInfo, camera::types::CameraError>> {
        if self.camera_denied() {
            return Ok(Err(camera::types::CameraError::PermissionDenied));
        }
        Ok(self
            .cameras
            .info(stream_id)
            .map(|info| camera::types::FrameInfo {
                width: info.width,
                height: info.height,
                fps: info.fps,
                format: camera::types::FrameFormat::Rgba8,
            })
            .map_err(camera_error))
    }

    fn start(
        &mut self,
        stream_id: u64,
    ) -> wasmtime::Result<Result<(), camera::types::CameraError>> {
        if self.camera_denied() {
            return Ok(Err(camera::types::CameraError::PermissionDenied));
        }
        Ok(self.cameras.start(stream_id).map_err(camera_error))
    }

    fn stop(&mut self, stream_id: u64) -> wasmtime::Result<Result<(), camera::types::CameraError>> {
        // Deliberately NOT behind the grant. Stopping is how an app turns the
        // camera off, and refusing it because the grant was revoked would
        // leave the device running with no way to stop it.
        Ok(self.cameras.stop(stream_id).map_err(camera_error))
    }

    fn read(
        &mut self,
        stream_id: u64,
    ) -> wasmtime::Result<Result<Option<camera::types::Frame>, camera::types::CameraError>> {
        if self.camera_denied() {
            return Ok(Err(camera::types::CameraError::PermissionDenied));
        }
        Ok(self
            .cameras
            .read(stream_id)
            .map(|frame| {
                frame.map(|frame| camera::types::Frame {
                    bytes: frame.bytes,
                    width: frame.width,
                    height: frame.height,
                    elapsed_millis: frame.elapsed_millis,
                })
            })
            .map_err(camera_error))
    }

    fn close(
        &mut self,
        stream_id: u64,
    ) -> wasmtime::Result<Result<(), camera::types::CameraError>> {
        // Like `stop`, always allowed: releasing the device is never the thing
        // a person needs protecting from.
        Ok(self.cameras.close(stream_id).map_err(camera_error))
    }
}

/// Printing (Phase 4 only). Any app with a window may ask: nothing reaches
/// a printer unless the person says so in the system's own dialog.
impl crate::phase4_gui_bindings::krate::ui::print::Host for Phase3GuiHost {
    fn window(
        &mut self,
        window: u64,
        title: String,
    ) -> wasmtime::Result<
        Result<
            crate::phase4_gui_bindings::krate::ui::print::PrintOutcome,
            crate::phase4_gui_bindings::krate::ui::types::UiError,
        >,
    > {
        let page = match self.capture_print_page(window) {
            Ok(page) => page,
            Err(err) => return Ok(Err(crate::phase4_gui_host::error_to_phase4(err))),
        };
        Ok(print_now(&title, &[page]))
    }

    fn add_page(
        &mut self,
        window: u64,
    ) -> wasmtime::Result<Result<u32, crate::phase4_gui_bindings::krate::ui::types::UiError>> {
        if self.print_pages_mut().len() >= crate::print_host::MAX_PAGES {
            return Ok(Err(
                crate::phase4_gui_bindings::krate::ui::types::UiError::Platform(format!(
                    "a document holds at most {} pages; finish this one first",
                    crate::print_host::MAX_PAGES
                )),
            ));
        }
        let page = match self.capture_print_page(window) {
            Ok(page) => page,
            Err(err) => return Ok(Err(crate::phase4_gui_host::error_to_phase4(err))),
        };
        let pages = self.print_pages_mut();
        pages.push(page);
        Ok(Ok(pages.len() as u32))
    }

    fn finish(
        &mut self,
        window: u64,
        title: String,
    ) -> wasmtime::Result<
        Result<
            crate::phase4_gui_bindings::krate::ui::print::PrintOutcome,
            crate::phase4_gui_bindings::krate::ui::types::UiError,
        >,
    > {
        // The window must still be the app's, like every other call.
        if let Err(err) = self.capture_print_page(window).map(|_| ()) {
            return Ok(Err(crate::phase4_gui_host::error_to_phase4(err)));
        }
        let pages = std::mem::take(self.print_pages_mut());
        if pages.is_empty() {
            return Ok(Err(
                crate::phase4_gui_bindings::krate::ui::types::UiError::Platform(
                    "no pages were added to print".to_string(),
                ),
            ));
        }
        Ok(print_now(&title, &pages))
    }
}

fn print_now(
    title: &str,
    pages: &[crate::print_host::Page],
) -> Result<
    crate::phase4_gui_bindings::krate::ui::print::PrintOutcome,
    crate::phase4_gui_bindings::krate::ui::types::UiError,
> {
    use crate::print_host::Outcome;
    let document = crate::print_host::pdf(title, pages);
    match crate::print_host::print(title, &document) {
        Ok(Outcome::Printed) => {
            Ok(crate::phase4_gui_bindings::krate::ui::print::PrintOutcome::Printed)
        }
        Ok(Outcome::Cancelled) => {
            Ok(crate::phase4_gui_bindings::krate::ui::print::PrintOutcome::Cancelled)
        }
        Ok(Outcome::OpenedInViewer) => {
            Ok(crate::phase4_gui_bindings::krate::ui::print::PrintOutcome::OpenedInViewer)
        }
        Err(why) => Err(crate::phase4_gui_bindings::krate::ui::types::UiError::Platform(why)),
    }
}

/// Speaking aloud (Phase 4 only; Phase 3 is frozen as shipped). Speech is
/// sound, so it is `audio.playback`'s to allow -- the consent line a person
/// already read says this app makes sound. `stop` is always allowed, like
/// closing a camera: silencing an app is never what needs protecting.
impl crate::phase4_gui_bindings::krate::speech::synthesis::Host for Phase3GuiHost {
    fn voices(
        &mut self,
    ) -> wasmtime::Result<
        Result<
            Vec<crate::phase4_gui_bindings::krate::speech::synthesis::Voice>,
            crate::phase4_gui_bindings::krate::speech::synthesis::SayError,
        >,
    > {
        use crate::phase4_gui_bindings::krate::speech::synthesis as wit;
        if !self.may_make_sound() {
            return Ok(Err(wit::SayError::PermissionDenied));
        }
        Ok(self
            .speech_out
            .voices()
            .map(|voices| {
                voices
                    .into_iter()
                    .map(|v| wit::Voice {
                        id: v.id,
                        name: v.name,
                        language: v.language,
                    })
                    .collect()
            })
            .map_err(say_error))
    }

    fn say(
        &mut self,
        text: String,
        voice: Option<String>,
        rate: Option<f32>,
    ) -> wasmtime::Result<Result<(), crate::phase4_gui_bindings::krate::speech::synthesis::SayError>>
    {
        use crate::phase4_gui_bindings::krate::speech::synthesis as wit;
        if !self.may_make_sound() {
            return Ok(Err(wit::SayError::PermissionDenied));
        }
        Ok(self
            .speech_out
            .say(&text, voice.as_deref(), rate)
            .map_err(say_error))
    }

    fn stop(
        &mut self,
    ) -> wasmtime::Result<Result<(), crate::phase4_gui_bindings::krate::speech::synthesis::SayError>>
    {
        self.speech_out.stop();
        Ok(Ok(()))
    }

    fn speaking(
        &mut self,
    ) -> wasmtime::Result<
        Result<bool, crate::phase4_gui_bindings::krate::speech::synthesis::SayError>,
    > {
        use crate::phase4_gui_bindings::krate::speech::synthesis as wit;
        if !self.may_make_sound() {
            return Ok(Err(wit::SayError::PermissionDenied));
        }
        Ok(Ok(self.speech_out.speaking()))
    }
}

impl Phase3GuiHost {
    fn may_make_sound(&self) -> bool {
        self.runtime
            .guard()
            .check(&UapiCall::Audio(AudioCall::Playback))
            .is_ok()
    }
}

fn say_error(err: SayError) -> crate::phase4_gui_bindings::krate::speech::synthesis::SayError {
    use crate::phase4_gui_bindings::krate::speech::synthesis::SayError as Wit;
    match err {
        SayError::InvalidRequest(why) => Wit::InvalidRequest(why),
        SayError::Unsupported(why) => Wit::Unsupported(why),
        SayError::Platform(why) => Wit::Platform(why),
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
#[cfg(not(any(target_os = "android", target_os = "ios", target_arch = "wasm32")))]
/// The native save dialog, returning where the person wants the file.
///
/// `save_file` rather than `pick_file`: a save dialog lets someone name a file
/// that does not exist yet, which is the whole point, and an open dialog
/// cannot.
pub(crate) fn save_file_on_host(
    title: &str,
    suggested: &str,
    filter: &str,
) -> Result<Option<std::path::PathBuf>, String> {
    let mut dialog = rfd::FileDialog::new();
    if !title.is_empty() {
        dialog = dialog.set_title(title);
    }
    if !suggested.is_empty() {
        dialog = dialog.set_file_name(suggested);
    }
    let extensions: Vec<&str> = filter
        .split(',')
        .map(str::trim)
        .filter(|ext| !ext.is_empty())
        .collect();
    if !extensions.is_empty() {
        dialog = dialog.add_filter("Supported files", &extensions);
    }
    Ok(dialog.save_file())
}

/// A reminder id an app may use: 1 to 64 of letters, digits, `-` and `_`.
pub(crate) fn valid_reminder_id(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Headless, `KRATE_BLUETOOTH=heart`: a stand-in heart-rate strap is
/// nearby (service 180D, measurement 2A37; battery 180F/2A19 at 87%).
fn fake_heart() -> bool {
    std::env::var("KRATE_BLUETOOTH").is_ok_and(|v| v == "heart")
}

/// A Bluetooth UUID in its short form when it is one of the standard ones
/// ("0000180D-0000-1000-8000-00805F9B34FB" is "180D"), upper case.
pub(crate) fn short_uuid(uuid: &str) -> String {
    let upper = uuid.trim().to_ascii_uppercase();
    match upper.strip_suffix("-0000-1000-8000-00805F9B34FB") {
        Some(head) if head.len() == 8 && head.starts_with("0000") => head[4..].to_string(),
        _ => upper,
    }
}

/// Where a save on a phone is written before it goes to the save sheet: a
/// fresh folder of its own in the app's temporary space (so two saves of
/// one name never meet), under the name the app suggested, with the
/// filter's first extension when the name has none.
#[cfg(any(target_os = "ios", target_os = "android", test))]
pub(crate) fn export_path(suggested: &str, filter: &str) -> std::path::PathBuf {
    let mut name: String = suggested
        .chars()
        .map(|c| {
            if c == '/' || c == '\\' || c.is_control() {
                '-'
            } else {
                c
            }
        })
        .collect::<String>()
        .trim()
        .trim_start_matches('.')
        .to_string();
    if name.is_empty() {
        name = "Untitled".to_string();
    }
    let ext = filter
        .split(',')
        .map(|e| e.trim().trim_start_matches('.'))
        .find(|e| !e.is_empty() && e.chars().all(|c| c.is_ascii_alphanumeric()));
    if let Some(ext) = ext {
        if std::path::Path::new(&name).extension().is_none() {
            name = format!("{name}.{ext}");
        }
    }
    static SAVES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SAVES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir()
        .join("krate-saves")
        .join(format!("{}-{n}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    dir.join(name)
}

// Desktop only, like its neighbour: on the phones and in a browser `rfd`
// is not linked at all, and an ungated call broke the Android build.
#[cfg(not(any(target_os = "android", target_os = "ios", target_arch = "wasm32")))]
pub(crate) fn choose_file_on_host(
    title: &str,
    filter: &str,
) -> Result<Option<std::path::PathBuf>, String> {
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
    fn a_phone_save_keeps_the_apps_name_and_gets_an_extension() {
        let path = export_path("Sketch", "png,jpg");
        assert_eq!(path.file_name().unwrap(), "Sketch.png");
        assert_eq!(
            export_path("notes.txt", "md").file_name().unwrap(),
            "notes.txt"
        );
        assert_eq!(export_path("", "").file_name().unwrap(), "Untitled");
        let sneaky = export_path("../../etc/x", "");
        assert_eq!(
            sneaky
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .file_name()
                .unwrap(),
            "krate-saves"
        );
        assert_ne!(
            export_path("a", "").parent(),
            export_path("a", "").parent(),
            "two saves never meet"
        );
    }

    /// "You cannot" and "this Mac cannot" must not reach a GUI app as the
    /// same error (CP2).
    ///
    /// The Phase 3 world already has `unsupported(string)` beside
    /// `permission-denied`, and this mapping already keeps them apart -- but
    /// nothing pinned it, and the two are one careless match arm away from
    /// collapsing. The clipboard is the live case: wired in the Linux
    /// adapter, `Unsupported` on macOS. An app told "permission denied" for
    /// it would prompt the person to grant something no grant can fix.
    ///
    /// The message has to survive too: "the host cannot support this UI
    /// feature yet" is only useful if it says which feature.
    #[test]
    fn a_platform_gap_does_not_reach_the_app_as_a_permission_denial() {
        let refused = dispatch_error_to_ui_error(UiDispatchError::PermissionDenied);
        assert!(
            matches!(refused, ui::types::UiError::PermissionDenied),
            "a policy refusal is a permission denial: {refused:?}"
        );

        let gap = dispatch_error_to_ui_error(UiDispatchError::Adapter(
            UiAdapterError::Unsupported("clipboard is not wired on this platform".into()),
        ));
        match gap {
            ui::types::UiError::Unsupported(why) => assert!(
                why.contains("clipboard"),
                "the reason must name the feature, or an app cannot report it: {why}"
            ),
            other => panic!(
                "a platform gap must reach the app as unsupported, not as {other:?} -- \
                 an app told 'permission denied' will ask the person to grant \
                 something no grant can fix"
            ),
        }
    }

    #[test]
    fn scroll_offset_clamps_to_content_extent() {
        // Content 192 tall in a 120 viewport: max offset 72.
        assert_eq!(clamped_scroll_offset(0.0, 30.0, 192.0, 120.0), 30.0);
        assert_eq!(clamped_scroll_offset(60.0, 30.0, 192.0, 120.0), 72.0);
        assert_eq!(clamped_scroll_offset(10.0, -30.0, 192.0, 120.0), 0.0);
        // Content shorter than the viewport never scrolls.
        assert_eq!(clamped_scroll_offset(0.0, 30.0, 80.0, 120.0), 0.0);
    }

    /// Enter on a focused button is a press of it (IC-743, test 1548).
    ///
    /// Measured before this existed: hello-gui's button answered the
    /// pointer and ignored Enter, because the runtime delivered a raw key
    /// and nothing else. The app receives the key first, then a press and a
    /// release aimed at the button, exactly as a pointer would deliver them.
    /// A threaded app's worker presents (IC-929): the frame waits for the
    /// window's own thread, which lowers it on its next turn; a worker asks
    /// nothing of the window itself and reads the density the main thread saw.
    #[test]
    fn a_frame_a_worker_presents_is_lowered_on_the_main_threads_turn() {
        let mut host = headless_host();
        ui::window::Host::create(
            &mut host,
            "app".to_string(),
            ui::types::WindowSize {
                width: 200,
                height: 100,
            },
        )
        .expect("create call")
        .expect("a window");
        let window = *host.windows.first().expect("window id");
        // Nothing has come from a worker: the main thread's turn is free.
        host.main_turn();
        assert!(!host.screen.used.get());

        struct Across<'a>(&'a Phase3GuiHost);
        // SAFETY: the test thread waits for the worker; the two never touch
        // the host at once, as core_app's host lock guarantees in a real run.
        unsafe impl Send for Across<'_> {}
        unsafe impl Sync for Across<'_> {}
        impl<'a> Across<'a> {
            fn host(&self) -> &'a Phase3GuiHost {
                self.0
            }
        }
        host.screen.scales.borrow_mut().insert(window, 2.0);
        let across = Across(&host);
        let worker_scale = std::thread::scope(|s| {
            s.spawn(|| {
                let host = across.host();
                assert!(!host.screen.on_main());
                host.hand_frame_to_main(window, true);
                host.hand_frame_to_main(window, false);
                // A worker's turn lowers nothing: only the window's thread can.
                host.main_turn();
                host.canvas_backing_scale(window)
            })
            .join()
            .expect("worker")
        });
        assert_eq!(worker_scale, 2.0, "the density the main thread last saw");
        assert_eq!(
            *host.screen.unsynced.borrow(),
            vec![(window, true)],
            "one frame waits for the window, still worth a shot"
        );
        host.main_turn();
        assert!(host.screen.unsynced.borrow().is_empty());
        assert_eq!(
            host.screen.scales.borrow().get(&window).copied(),
            Some(host.dispatcher().window_scale(window)),
            "the main thread's turn noted the window's own density"
        );
    }

    #[test]
    fn enter_on_a_focused_button_arrives_as_a_press_and_a_release() {
        let mut host = headless_host();
        let window_id = ui::window::Host::create(
            &mut host,
            "app".to_string(),
            ui::types::WindowSize {
                width: 200,
                height: 100,
            },
        )
        .expect("create call")
        .expect("a window");
        let node = wit_node(ui::types::WidgetKind::Button, None);
        let widget_raw = node.id;
        ui::tree::Host::set_root(&mut host, window_id, node)
            .expect("set_root call")
            .expect("a button root");
        let window = *host.windows.first().expect("window id");
        let widget = WidgetId::new(widget_raw).expect("widget id");
        host.dispatcher().focus_node(window, widget).expect("focus");
        for pressed in [true, false] {
            host.dispatcher()
                .route_key_event(crate::phase3_ui::KeyRouteRequest {
                    window,
                    key: "Enter".to_string(),
                    pressed,
                    modifiers: Default::default(),
                })
                .expect("key route");
        }
        let mut seen = Vec::new();
        for _ in 0..8 {
            match ui::events::Host::wait(&mut host, Some(5)).expect("wait call") {
                Some(event) => seen.push(event),
                None => break,
            }
        }
        let keys = seen
            .iter()
            .filter(|e| matches!(e, ui::types::Event::Key(_)))
            .count();
        assert!(keys >= 1, "the app still receives the key itself: {seen:?}");
        let presses: Vec<(bool, Option<u64>)> = seen
            .iter()
            .filter_map(|e| match e {
                ui::types::Event::Pointer(p) => Some((p.pressed, p.widget)),
                _ => None,
            })
            .collect();
        assert_eq!(
            presses,
            vec![(true, Some(widget_raw)), (false, Some(widget_raw))],
            "Enter on the focused button must arrive as one press and one release of that button: {seen:?}"
        );
        let key_at = seen
            .iter()
            .position(|e| matches!(e, ui::types::Event::Key(_)))
            .unwrap();
        let press_at = seen
            .iter()
            .position(|e| matches!(e, ui::types::Event::Pointer(p) if p.pressed))
            .unwrap();
        assert!(
            key_at < press_at,
            "the key comes first, the synthesized press after it"
        );
    }

    /// Drive a headless host's usability plan until it reports, or a
    /// budget runs out. The driver advances on `wait`, one step per
    /// STEP_INTERVAL.
    fn drive_until_task_reports(host: &mut Phase3GuiHost) -> crate::usability::UsabilityReport {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            let _ = ui::events::Host::wait(host, Some(50));
            let report = host.usability_report().expect("a plan");
            if report.task.is_some() || std::time::Instant::now() > deadline {
                return report;
            }
            std::thread::sleep(std::time::Duration::from_millis(60));
        }
    }

    fn task_plan(task: crate::usability::Task) -> crate::usability::UsabilityPlan {
        crate::usability::UsabilityPlan {
            report_path: std::env::temp_dir()
                .join(format!("krate-task-test-{}.json", std::process::id())),
            check_resize: false,
            check_click: false,
            check_keyboard: false,
            check_stay_open: false,
            task: Some(task),
        }
    }

    fn step(field: &str, value: &str) -> crate::usability::TaskStep {
        let mut s = crate::usability::TaskStep::default();
        match field {
            "click" => s.click = Some(value.to_string()),
            "click_at" => s.click_at = Some(value.to_string()),
            "hold" => s.hold = Some(value.to_string()),
            "pad" => s.pad = Some(value.to_string()),
            "stick" => s.stick = Some(value.to_string()),
            "look" => s.look = Some(value.to_string()),
            "shoot" => s.shoot = Some(value.to_string()),
            "key" => s.key = Some(value.to_string()),
            "type" => s.type_text = Some(value.to_string()),
            "expect_text" => s.expect_text = Some(value.to_string()),
            "expect_no_text" => s.expect_no_text = Some(value.to_string()),
            _ => s.wait_ms = value.parse().ok(),
        }
        s
    }

    fn host_with_labelled_button(
        label: &str,
        plan: crate::usability::UsabilityPlan,
    ) -> Phase3GuiHost {
        let mut host = Phase3GuiHost::new(
            UapiGuard::new(Default::default()),
            Phase3HostUiMode::HeadlessDraft,
        )
        .expect("headless host")
        .with_usability(Some(plan));
        let window_id = ui::window::Host::create(
            &mut host,
            "app".to_string(),
            ui::types::WindowSize {
                width: 300,
                height: 200,
            },
        )
        .expect("create call")
        .expect("a window");
        let mut node = wit_node(ui::types::WidgetKind::Button, None);
        node.label = Some(label.to_string());
        ui::tree::Host::set_root(&mut host, window_id, node)
            .expect("set_root call")
            .expect("root");
        host
    }

    /// The task oracle (IC-743, test 1506): a task whose expectation is on
    /// screen completes; one that names a control that is not there fails
    /// and says what IS there.
    #[test]
    fn a_task_completes_when_its_expectation_is_on_screen_and_names_what_is_there_when_not() {
        let task = crate::usability::Task {
            name: "see the button".to_string(),
            steps: vec![step("expect_text", "Add item")],
        };
        let mut host = host_with_labelled_button("Add item", task_plan(task));
        let report = drive_until_task_reports(&mut host);
        assert_eq!(
            report.task,
            Some(crate::usability::Observation::Held),
            "{report:?}"
        );
        assert_eq!(report.task_name.as_deref(), Some("see the button"));
        assert_eq!(report.task_steps_done, 1);

        let task = crate::usability::Task {
            name: "press what is not there".to_string(),
            steps: vec![step("click", "Remove"), step("expect_text", "gone")],
        };
        let mut host = host_with_labelled_button("Add item", task_plan(task));
        let report = drive_until_task_reports(&mut host);
        match report.task {
            Some(crate::usability::Observation::Broke { detail }) => {
                assert!(detail.contains("step 1 of 2"), "{detail}");
                assert!(
                    detail.contains("\"Remove\""),
                    "the missing label is named: {detail}"
                );
                assert!(
                    detail.contains("\"Add item\""),
                    "and what is on screen is listed: {detail}"
                );
            }
            other => {
                panic!("a click on a control that is not there must break the task: {other:?}")
            }
        }
        assert_eq!(report.task_steps_done, 0);

        // An expectation that is not on screen breaks the task by itself.
        let task = crate::usability::Task {
            name: "expect what is not there".to_string(),
            steps: vec![step("expect_text", "nope")],
        };
        let mut host = host_with_labelled_button("Add item", task_plan(task));
        let report = drive_until_task_reports(&mut host);
        assert!(
            matches!(report.task, Some(crate::usability::Observation::Broke { ref detail }) if detail.contains("\"nope\"")),
            "{report:?}"
        );

        // And something that must NOT be on screen, but is, breaks it too.
        let task = crate::usability::Task {
            name: "expect the button gone".to_string(),
            steps: vec![step("expect_no_text", "Add item")],
        };
        let mut host = host_with_labelled_button("Add item", task_plan(task));
        let report = drive_until_task_reports(&mut host);
        assert!(
            matches!(report.task, Some(crate::usability::Observation::Broke { ref detail }) if detail.contains("still on screen")),
            "{report:?}"
        );

        // A click on a control that IS there is delivered, settles, and the
        // expectation after it is judged on the labels then on screen.
        let task = crate::usability::Task {
            name: "press the button".to_string(),
            steps: vec![
                step("click", "add item"),
                step("expect_no_text", "Remove"),
                step("expect_text", "Add"),
            ],
        };
        let mut host = host_with_labelled_button("Add item", task_plan(task));
        let report = drive_until_task_reports(&mut host);
        assert_eq!(
            report.task,
            Some(crate::usability::Observation::Held),
            "{report:?}"
        );
        assert_eq!(report.task_steps_done, 3);
    }

    /// An app that draws its own interface (a game) is driven by point and
    /// by held key: `click_at` presses the window where it is told, and
    /// `hold` keeps a key down for its time before letting it go.
    #[test]
    fn a_task_presses_a_point_and_holds_a_key_for_its_time() {
        let shot = std::env::temp_dir().join(format!("krate-task-shot-{}.png", std::process::id()));
        let _ = std::fs::remove_file(&shot);
        let task = crate::usability::Task {
            name: "walk".to_string(),
            steps: vec![
                step("click_at", "0.25,0.5"),
                step("hold", "w 600"),
                step("shoot", shot.to_str().unwrap()),
                step("expect_no_text", "a crash dialog"),
            ],
        };
        let mut host = host_with_labelled_button("Play", task_plan(task));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let mut presses = Vec::new();
        let mut keys = Vec::new();
        let report = loop {
            match ui::events::Host::wait(&mut host, Some(50)).expect("wait call") {
                Some(ui::types::Event::Pointer(p)) => presses.push((p.pressed, p.x, p.y)),
                Some(ui::types::Event::Key(k)) => {
                    keys.push((k.key.clone(), k.pressed, std::time::Instant::now()))
                }
                _ => {}
            }
            let report = host.usability_report().expect("a plan");
            if report.task.is_some() || std::time::Instant::now() > deadline {
                break report;
            }
        };
        assert_eq!(
            report.task,
            Some(crate::usability::Observation::Held),
            "{report:?}"
        );
        assert_eq!(report.task_steps_done, 4);
        let png = std::fs::read(&shot).expect("the shoot step wrote its picture");
        assert!(png.starts_with(b"\x89PNG"), "a PNG");
        let _ = std::fs::remove_file(&shot);
        let pressed: Vec<bool> = presses.iter().map(|p| p.0).collect();
        assert_eq!(pressed, vec![true, false], "one press, then its release");
        // The window is 300 wide (host_with_labelled_button): a quarter in.
        assert!(
            (presses[0].1 - 75.0).abs() < 1.0,
            "pressed at the point asked: {presses:?}"
        );
        assert_eq!(
            keys.iter().map(|k| (k.0.as_str(), k.1)).collect::<Vec<_>>(),
            vec![("w", true), ("w", false)],
            "the key goes down, then up"
        );
        let held = keys[1].2.duration_since(keys[0].2);
        assert!(
            held >= std::time::Duration::from_millis(550),
            "the key stays down for its time: {held:?}"
        );
        // The run's deadline grows by the script's own time: a wait, a held
        // key (at least a settle), a settle per press, nothing for a look.
        let long = crate::usability::Task {
            name: "reach the menu".to_string(),
            steps: vec![
                step("wait_ms", "20000"),
                step("click_at", "0.5,0.5"),
                step("hold", "w 3000"),
                step("expect_no_text", "x"),
            ],
        };
        assert_eq!(
            long.scripted_time(std::time::Duration::from_millis(1500)),
            std::time::Duration::from_millis(24_500)
        );
        let bad = crate::usability::Task {
            name: "nowhere".to_string(),
            steps: vec![step("click_at", "1.5,0"), step("expect_text", "x")],
        };
        assert!(bad.validate().unwrap_err().contains("between 0 and 1"));
        let bad = crate::usability::Task {
            name: "forever".to_string(),
            steps: vec![step("hold", "w"), step("expect_text", "x")],
        };
        assert!(bad.validate().unwrap_err().contains("key milliseconds"));
    }

    /// IC-937: a driven headless run holds the pointer when the app asks, and
    /// a `look` step gives it raw motion over the step's time. (A plain
    /// headless run still refuses: an_app_asks_about_the_mouse_and_a_headless_capture_is_refused.)
    #[test]
    fn a_task_turns_a_held_pointer_in_a_headless_run() {
        let task = crate::usability::Task {
            name: "look around".to_string(),
            steps: vec![
                step("look", "120 -60 400"),
                step("expect_no_text", "a crash dialog"),
            ],
        };
        let mut host = host_with_labelled_button("Play", task_plan(task));
        let window = host.windows.first().expect("a window").get();
        // Not held: a look moves nothing.
        assert!(!host.pointer_captured(window));
        host.pointer_capture(window, true)
            .expect("a headless run can hold the pointer");
        assert!(host.pointer_captured(window));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let (mut dx, mut dy, mut takes) = (0.0f32, 0.0f32, 0);
        let report = loop {
            let _ = ui::events::Host::wait(&mut host, Some(20));
            let (x, y) = host.pointer_take_motion(window);
            if x != 0.0 || y != 0.0 {
                takes += 1;
            }
            dx += x;
            dy += y;
            let report = host.usability_report().expect("a plan");
            if report.task.is_some() || std::time::Instant::now() > deadline {
                break report;
            }
        };
        assert_eq!(
            report.task,
            Some(crate::usability::Observation::Held),
            "{report:?}"
        );
        assert!(
            (dx - 120.0).abs() < 0.01 && (dy + 60.0).abs() < 0.01,
            "{dx},{dy}"
        );
        assert!(
            takes > 1,
            "the motion came over several frames, not one jump"
        );
        host.pointer_capture(window, false).unwrap();
        assert!(!host.pointer_captured(window));
    }

    /// IC-936: a driven run plugs in a stand-in controller, holds its button
    /// and stick for their times, and lets go.
    #[test]
    fn a_task_holds_a_stand_in_gamepad_button_and_stick() {
        let task = crate::usability::Task {
            name: "jump and run".to_string(),
            steps: vec![
                step("pad", "south 400"),
                step("stick", "left-y 0.75 400"),
                step("expect_no_text", "a crash dialog"),
            ],
        };
        let mut host = host_with_labelled_button("Play", task_plan(task));
        assert!(
            !host.gamepads.borrow_mut().connected(),
            "no pad before the script"
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let (mut saw_south, mut saw_stick) = (false, false);
        let report = loop {
            let _ = ui::events::Host::wait(&mut host, Some(20));
            saw_south |= ui::events::Host::gamepad_held(&mut host, "south".into()).unwrap();
            let y = ui::events::Host::gamepad_axis(&mut host, "left-y".into()).unwrap();
            saw_stick |= (y - 0.75).abs() < 1e-6;
            let report = host.usability_report().expect("a plan");
            if report.task.is_some() || std::time::Instant::now() > deadline {
                break report;
            }
        };
        assert_eq!(
            report.task,
            Some(crate::usability::Observation::Held),
            "{report:?}"
        );
        assert!(saw_south, "south was held");
        assert!(saw_stick, "the stick was pushed to 0.75");
        assert!(ui::events::Host::gamepad_connected(&mut host).unwrap());
        assert!(
            !ui::events::Host::gamepad_held(&mut host, "south".into()).unwrap(),
            "let go"
        );
        assert_eq!(
            ui::events::Host::gamepad_axis(&mut host, "left-y".into()).unwrap(),
            0.0
        );
        let bad = crate::usability::Task {
            name: "triangle".to_string(),
            steps: vec![step("pad", "triangle 100"), step("expect_text", "x")],
        };
        assert!(bad.validate().unwrap_err().contains("gamepad button"));
    }

    /// A row of a list answers Enter the way it answers a press: the pointer
    /// step calls such a row clickable, so the keyboard step must reach it.
    #[test]
    fn enter_on_a_focused_list_row_arrives_as_a_press() {
        let mut host = headless_host();
        let window_id = ui::window::Host::create(
            &mut host,
            "app".to_string(),
            ui::types::WindowSize {
                width: 200,
                height: 100,
            },
        )
        .expect("create call")
        .expect("a window");
        let mut list = wit_node(ui::types::WidgetKind::ListView, None);
        list.id = 1;
        ui::tree::Host::set_root(&mut host, window_id, list)
            .expect("set_root call")
            .expect("root");
        let mut row = wit_node(ui::types::WidgetKind::Text, None);
        row.id = 2;
        row.parent = Some(1);
        ui::tree::Host::upsert_node(&mut host, window_id, row)
            .expect("upsert call")
            .expect("row");
        let window = *host.windows.first().expect("window id");
        host.dispatcher()
            .focus_node(window, WidgetId::new(2).unwrap())
            .expect("focus");
        host.dispatcher()
            .route_key_event(crate::phase3_ui::KeyRouteRequest {
                window,
                key: "Space".to_string(),
                pressed: true,
                modifiers: Default::default(),
            })
            .expect("key route");
        let mut presses = Vec::new();
        for _ in 0..8 {
            match ui::events::Host::wait(&mut host, Some(5)).expect("wait call") {
                Some(ui::types::Event::Pointer(p)) => presses.push((p.pressed, p.widget)),
                Some(_) => {}
                None => break,
            }
        }
        assert_eq!(
            presses,
            vec![(true, Some(2)), (false, Some(2))],
            "Space on a focused list row presses that row"
        );
    }

    /// A key on a control that is not activatable, and a key that is not
    /// Enter or Space, synthesize nothing.
    #[test]
    fn other_keys_and_other_widgets_synthesize_no_press() {
        let mut host = headless_host();
        let window_id = ui::window::Host::create(
            &mut host,
            "app".to_string(),
            ui::types::WindowSize {
                width: 200,
                height: 100,
            },
        )
        .expect("create call")
        .expect("a window");
        let node = wit_node(ui::types::WidgetKind::Text, None);
        let widget_raw = node.id;
        ui::tree::Host::set_root(&mut host, window_id, node)
            .expect("set_root call")
            .expect("root");
        let window = *host.windows.first().expect("window id");
        let widget = WidgetId::new(widget_raw).expect("widget id");
        host.dispatcher().focus_node(window, widget).expect("focus");
        for key in ["Enter", "a"] {
            host.dispatcher()
                .route_key_event(crate::phase3_ui::KeyRouteRequest {
                    window,
                    key: key.to_string(),
                    pressed: true,
                    modifiers: Default::default(),
                })
                .expect("key route");
        }
        let mut pointer = 0;
        for _ in 0..8 {
            match ui::events::Host::wait(&mut host, Some(5)).expect("wait call") {
                Some(ui::types::Event::Pointer(_)) => pointer += 1,
                Some(_) => {}
                None => break,
            }
        }
        assert_eq!(
            pointer, 0,
            "a text node is not activatable, and 'a' activates nothing"
        );
    }

    /// `save-file` refuses without the capability, and cancels when headless.
    ///
    /// Both halves matter and they fail in opposite directions. Without the
    /// guard an app could pop a save dialog its manifest never asked for --
    /// the exact hole K-086 closed for the other dialogs. Without the headless
    /// rule, `--shoot`, check-app and CI would block on a native dialog that
    /// nobody can click, which reads as a hung app rather than a missing one.
    ///
    /// A default guard grants nothing, so the first call must be refused; that
    /// also proves the check runs BEFORE the headless shortcut, which is the
    /// order that keeps an unpermitted app from learning anything at all.
    #[test]
    fn asking_where_to_save_is_guarded_and_cancels_when_headless() {
        let mut host = headless_host();

        let refused = host.ask_where_to_save("Save", "note.txt", "txt");
        assert!(
            matches!(refused, Err(ui::types::UiError::PermissionDenied)),
            "an app without ui.dialog:file-save must be refused, not shown a \
             dialog: {refused:?}"
        );
    }

    /// G17: where the organisation's rules for this app say no printing
    /// and no pictures, a print and a picture of the window are refused;
    /// without them neither is.
    #[test]
    fn an_organisations_app_rules_stop_printing_and_pictures() {
        let ceiling = krate_policy::OrgCeiling {
            deny: Vec::new(),
            network: None,
            calls: vec![
                krate_policy::PRINT_CALL.to_string(),
                krate_policy::SCREEN_CAPTURE_CALL.to_string(),
            ],
        };
        let policy = krate_policy::SessionPolicy::from_cli_grants(&["ui.window:create".into()])
            .expect("policy")
            .with_organisation(ceiling);
        let kept = Phase3GuiHost::new(UapiGuard::new(policy), Phase3HostUiMode::HeadlessDraft)
            .expect("headless host");
        assert!(matches!(
            kept.capture_print_page(1),
            Err(ui::types::UiError::PermissionDenied)
        ));
        let dir = tempfile::tempdir().expect("dir");
        let shot = dir.path().join("shot.png");
        let window = kept
            .dispatcher()
            .create_window(
                krate_adapter_common::ui::WindowOptions::new(
                    "Payroll",
                    krate_adapter_common::ui::WindowSize::new(320, 240).expect("size"),
                )
                .expect("options"),
            )
            .expect("window");
        assert!(matches!(
            kept.render_window_png(window, 1.0, &shot),
            Err(UiDispatchError::PermissionDenied)
        ));
        assert!(kept
            .dispatcher()
            .organisation_blocks_call(krate_policy::SCREEN_CAPTURE_CALL));
        let plain = headless_host();
        assert!(!matches!(
            plain.capture_print_page(1),
            Err(ui::types::UiError::PermissionDenied)
        ));
        assert!(!matches!(
            plain.render_window_png(window, 1.0, &shot),
            Err(UiDispatchError::PermissionDenied)
        ));
        assert!(!plain
            .dispatcher()
            .organisation_blocks_call(krate_policy::SCREEN_CAPTURE_CALL));
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
    fn a_3d_scene_paces_its_frames_like_a_canvas_does() {
        // A guest game loop calls present with no sleep of its own. The 2D
        // path has always waited out the frame budget; the 3D path did not,
        // so a 3D app free-ran at roughly three times the display rate and
        // spent a third of a core drawing frames nothing would show (K-399).
        //
        // Timed rather than read off the source: the question is whether
        // presenting twice in a row actually takes a frame, which is the thing
        // that was wrong and the thing a future edit could quietly undo.
        let (mut host, window, widget) = host_with_canvas_widget();
        let scene = gfx::scene3d::Host::bind(&mut host, window, widget)
            .expect("bind call")
            .expect("a canvas widget binds a scene");

        let present = |host: &mut Phase3GuiHost| {
            gfx::scene3d::Host::present(host, scene)
                .expect("present call")
                .expect("present succeeds");
        };

        // Two warm-up frames: the first has no previous frame to pace
        // against, and the second is the first that actually renders on a
        // freshly created backend.
        present(&mut host);
        present(&mut host);

        // A RUN of frames, not one. The pace stamp is taken before the
        // render, so a frame whose render overruns the budget leaves the
        // frame after it nothing to wait for. That is correct pacing, and it
        // is what happens on a slow shared runner: the warm-up frame that
        // pays for device and pipeline setup overran, and the single frame
        // this test used to time was exactly the one that got no wait. It
        // failed three times in two days on the macOS lane and never here.
        //
        // Over N consecutive presents the stamps are at least a budget apart
        // whether or not any one render was slow -- a slow render only pushes
        // the next stamp further out -- so the total is bounded below by
        // (N - 1) budgets no matter which frame the setup cost lands on.
        // Unpaced, the whole run was a few milliseconds.
        const FRAMES: u32 = 6;
        let started = std::time::Instant::now();
        for _ in 0..FRAMES {
            present(&mut host);
        }
        let elapsed = started.elapsed();

        // Most of a budget per frame, not all of it: the wait ends by
        // yielding and a loaded machine can land a little under.
        let floor = std::time::Duration::from_millis(12) * (FRAMES - 1);
        assert!(
            elapsed >= floor,
            "{FRAMES} presents should wait out at least {floor:?} between them, took {elapsed:?}"
        );
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
    fn the_second_unanswered_close_request_is_the_runtime_s_to_honour() {
        // K-121: a machine was power-cycled because an app that ignored
        // CloseRequested could not be shut at all. The contract is two
        // presses: the first is the app's chance to save on the way out, the
        // second means the person's patience has run out and the runtime
        // ends the process. This pins the threshold; the exits themselves
        // sit in poll, wait and both presents and cannot run under a test.
        let (host, window, _widget) = host_with_canvas_widget();
        assert!(
            !host.close_ignored_by_guest(),
            "an app that was never asked to close is not ignoring anything"
        );
        host.note_close_request(&ui::types::Event::CloseRequested(window));
        assert!(
            !host.close_ignored_by_guest(),
            "the first request belongs to the app -- saving on the way out"
        );
        host.note_close_request(&ui::types::Event::CloseRequested(window));
        assert!(
            host.close_ignored_by_guest(),
            "two unanswered requests mean the runtime closes on the app's behalf"
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
    fn a_spent_budget_ends_a_game_loop_that_only_polls() {
        // The budget has to reach a frame loop, not just an event loop. A game
        // drives itself with `poll` and `key-held` and never calls `wait`, and
        // `wait` used to be the only place that asked whether the budget was
        // spent -- so every game was silently exempt from it. A finished game
        // wrote its screenshot and then ran forever.
        let mut host = headless_host();
        use ui::window::Host as _;
        let _window = host
            .create(
                "game".to_string(),
                ui::types::WindowSize {
                    width: 100,
                    height: 100,
                },
            )
            .expect("create call")
            .expect("a window");

        let first = ui::events::Host::poll(&mut host).expect("poll call");
        assert!(
            !matches!(first, Some(ui::types::Event::CloseRequested(_))),
            "the budget must not fire on the very first poll"
        );

        host.headless_started.set(Some(
            std::time::Instant::now() - HEADLESS_RUN_BUDGET - std::time::Duration::from_millis(1),
        ));

        let event = ui::events::Host::poll(&mut host).expect("poll call");
        assert!(
            matches!(event, Some(ui::types::Event::CloseRequested(_))),
            "a spent budget must close a loop that only ever polls: {event:?}"
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
    fn a_stepped_clock_holds_the_shot_for_a_fixed_number_of_frames() {
        // K-721. `cubes` advances its spin by real elapsed time, so a shot
        // taken 400 real milliseconds in caught it at whatever angle this
        // machine happened to reach -- ten shots of one binary differed from
        // each other by up to 1.1% against a 0.5% tolerance, and the macOS
        // replay lane went red on about half of all pushes with no code change
        // behind it. The cure is to stop counting the delay in real time: on a
        // stepped clock the delay is a frame count, and 400 ms at 16 ms a frame
        // is frame 25 on every machine however fast it is.
        //
        // What this asserts is exactly that: the delay does not elapse because
        // real time passed, only because frames did. So the assertions sleep
        // far longer than the delay first, and still expect it to be pending.
        use krate_adapter_common::time::SteppedClock;

        let stepped = SteppedClock::new(16_000_000);
        let (mut host, _window, _widget) = host_with_canvas_widget();
        host.shoot_after = Some(std::time::Duration::from_millis(400));
        host.stepped_clock = Some(stepped.clone());

        // Real time is not the clock any more: well past 400 ms of it, with no
        // frame drawn, and the delay has not elapsed.
        std::thread::sleep(std::time::Duration::from_millis(500));
        assert!(
            !host.shoot_delay_elapsed(),
            "real time must not move a frame-counted delay, or the shot goes \
             back to depending on how fast the machine is"
        );

        // 400 ms of 16 ms frames is 25 frames. Frame 24 is still too early.
        for _ in 0..24 {
            stepped.end_frame();
        }
        assert_eq!(stepped.frames(), 24);
        assert!(
            !host.shoot_delay_elapsed(),
            "the delay must not elapse a frame early"
        );

        stepped.end_frame();
        assert!(
            host.shoot_delay_elapsed(),
            "25 frames of 16 ms is the requested 400 ms; the shot is due"
        );
    }

    #[test]
    fn presenting_a_canvas_advances_the_stepped_clock_by_one_frame() {
        // The other half of K-721: the clock only moves because the host moves
        // it. If present did not end the frame, an app on a stepped clock would
        // read time zero forever, never animate, and the shot after N frames
        // would be frame one -- a blank pose, not a deterministic one.
        use krate_adapter_common::time::SteppedClock;

        let stepped = SteppedClock::new(16_000_000);
        let (mut host, window, widget) = host_with_canvas_widget();
        host.stepped_clock = Some(stepped.clone());
        let canvas = gfx::canvas2d::Host::bind(&mut host, window, widget)
            .expect("bind call")
            .expect("a canvas widget binds");

        assert_eq!(stepped.frames(), 0, "no frame has been presented yet");
        for expected in 1..=3 {
            gfx::canvas2d::Host::present(&mut host, canvas)
                .expect("present call")
                .expect("present succeeds");
            assert_eq!(
                stepped.frames(),
                expected,
                "each presented frame must advance the stepped clock exactly once"
            );
        }
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
        assert_eq!(
            host.canvases.borrow().get(&canvas).unwrap().2.scale(),
            2.0,
            "K-425: the backing raster, not just the output PNG, must be 2x"
        );
        host.refit_canvas(canvas).expect("refit");
        assert_eq!(
            host.canvases.borrow().get(&canvas).unwrap().2.scale(),
            2.0,
            "K-425: refit must retain capture density on a headless adapter"
        );
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
        let image_size = {
            let images = host.images.borrow();
            let image = images
                .get(&(
                    host.window_id(window).unwrap(),
                    WidgetId::new(widget).unwrap(),
                ))
                .unwrap();
            (image.width, image.height)
        };
        assert_eq!(
            image_size,
            (400, 400),
            "no 200px canvas stretched into a 400px shot"
        );
        // And what painted it is written beside it (IC-743, test 1547):
        // the renderer, the scale, the pixel and logical sizes, and the
        // colour space, so a comparison can refuse a mismatched pair.
        let sidecar_path = shot_sidecar_path(&path);
        let sidecar: ShotSidecar = serde_json::from_str(
            &std::fs::read_to_string(&sidecar_path)
                .expect("the sidecar is written beside the shot"),
        )
        .expect("the sidecar is the runtime's own shape");
        assert_eq!(sidecar.schema, SHOT_SIDECAR_SCHEMA);
        assert_eq!(sidecar.renderer, SHARED_PAINTER);
        assert_eq!(sidecar.color_space, SHOT_COLOR_SPACE);
        assert!(sidecar.scale >= 1.0);
        assert_eq!(
            (sidecar.width, sidecar.height),
            (
                ((sidecar.logical_width as f32) * sidecar.scale).round() as u32,
                ((sidecar.logical_height as f32) * sidecar.scale).round() as u32
            ),
            "the pixel size is the logical size at the scale"
        );
        let png = std::fs::read(&path).expect("read the shot");
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        let png_width = u32::from_be_bytes([png[16], png[17], png[18], png[19]]);
        let png_height = u32::from_be_bytes([png[20], png[21], png[22], png[23]]);
        assert_eq!(
            (png_width, png_height),
            (sidecar.width, sidecar.height),
            "the sidecar describes the PNG it sits beside"
        );
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

#[cfg(test)]
mod notification_attribution_tests {
    use super::*;

    /// The sender of a notification is the app's consented name, and the
    /// call site never hands the guest's title in its place (IC-271).
    #[test]
    fn a_notification_is_attributed_to_the_app_not_to_its_own_title() {
        let host = Phase3GuiHost::new(UapiGuard::default(), Phase3HostUiMode::HeadlessDraft)
            .expect("headless host")
            .with_app_name(Some("Reading List".to_string()));
        assert_eq!(host.attribution(), "Reading List");

        let unnamed = Phase3GuiHost::new(UapiGuard::default(), Phase3HostUiMode::HeadlessDraft)
            .expect("headless host");
        assert_eq!(unnamed.attribution(), "a Krate app");

        // The call site itself. `show(title, body)` has the guest's title in
        // scope and the first version passed it as the sender; a behavioural
        // test cannot see that mistake without posting a real notification,
        // so the one line that matters is read.
        let source = include_str!("phase3_gui_host.rs");
        let show_at = source
            .find("impl ui::notify::Host for Phase3GuiHost")
            .expect("notify impl");
        let show = &source[show_at..show_at + 1500];
        assert!(
            show.contains("desktop_host::notify(&title, &body, &sender, granted)"),
            "show must pass the attribution as the sender"
        );
        assert!(
            !show.contains("notify(&title, &body, &title,"),
            "the guest's title must never be the sender line"
        );
    }
}
