//! The two host impls Phase 4 needs that Phase 3 cannot supply.
//!
//! Phase 4 differs from Phase 3 in exactly one thing: `widget-kind` has an
//! `overlay` case. That case is defined in `krate:ui/types` and travels
//! through `krate:ui/tree`, so those two interfaces generate their own Rust
//! types and need their own trait impls. Every other interface in the world is
//! mapped straight to the module Phase 3 generated (see
//! `phase4_gui_bindings`), so `Phase3GuiHost` already satisfies them.
//!
//! Nothing here reimplements a rule. The two phases cannot share a WIT record
//! -- that is the whole reason Phase 4 exists -- but they converge one step
//! later, on the host's own `WidgetNode`, which has the case. So this file
//! translates the enum and hands the parts to the same validator and the same
//! dispatcher Phase 3 uses. The behavioural difference between the phases is
//! one match arm.

use krate_adapter_common::ui::{BoxStyle, Color, Placement, TextStyle, WidgetKind, WidgetStyle};

use crate::phase3_gui_bindings::krate::gfx as gfx3;
use crate::phase3_gui_bindings::krate::ui as ui3;
use crate::phase3_gui_host::{widget_node_from_parts, Phase3GuiHost};
use crate::phase4_gui_bindings::krate::ui as ui4;

impl ui4::types::Host for Phase3GuiHost {}

impl ui4::tree::Host for Phase3GuiHost {
    fn set_root(
        &mut self,
        window: u64,
        root: ui4::types::WidgetNode,
    ) -> wasmtime::Result<Result<(), ui4::types::UiError>> {
        let root = match node_from_phase4(root) {
            Ok(root) => root,
            Err(err) => return Ok(Err(error_to_phase4(err))),
        };
        Ok(self.set_root_widget(window, root).map_err(error_to_phase4))
    }

    fn upsert_node(
        &mut self,
        window: u64,
        node: ui4::types::WidgetNode,
    ) -> wasmtime::Result<Result<(), ui4::types::UiError>> {
        let node = match node_from_phase4(node) {
            Ok(node) => node,
            Err(err) => return Ok(Err(error_to_phase4(err))),
        };
        Ok(self.upsert_widget(window, node).map_err(error_to_phase4))
    }

    fn remove_node(
        &mut self,
        window: u64,
        widget: u64,
    ) -> wasmtime::Result<Result<(), ui4::types::UiError>> {
        let result = ui3::tree::Host::remove_node(self, window, widget)?;
        Ok(result.map_err(error_to_phase4))
    }

    fn focus_node(
        &mut self,
        window: u64,
        widget: u64,
    ) -> wasmtime::Result<Result<(), ui4::types::UiError>> {
        let result = ui3::tree::Host::focus_node(self, window, widget)?;
        Ok(result.map_err(error_to_phase4))
    }

    fn set_enabled(
        &mut self,
        window: u64,
        widget: u64,
        enabled: bool,
    ) -> wasmtime::Result<Result<(), ui4::types::UiError>> {
        let result = ui3::tree::Host::set_enabled(self, window, widget, enabled)?;
        Ok(result.map_err(error_to_phase4))
    }
}

/// The dialogs. Three delegate to Phase 3; one is new.
///
/// `krate:ui/dialog` had to generate fresh for Phase 4 because it gained
/// `save-file`, which Phase 3 does not have. The other three are the same
/// functions with the same behaviour, so they hand straight over rather than
/// being written twice.
impl ui4::dialog::Host for Phase3GuiHost {
    fn message(
        &mut self,
        window: u64,
        title: String,
        body: String,
    ) -> wasmtime::Result<Result<(), ui4::types::UiError>> {
        let r = ui3::dialog::Host::message(self, window, title, body)?;
        Ok(r.map_err(error_to_phase4))
    }

    fn confirm(
        &mut self,
        window: u64,
        title: String,
        body: String,
    ) -> wasmtime::Result<Result<bool, ui4::types::UiError>> {
        let r = ui3::dialog::Host::confirm(self, window, title, body)?;
        Ok(r.map_err(error_to_phase4))
    }

    fn open_file(
        &mut self,
        window: u64,
        title: String,
        filter: String,
    ) -> wasmtime::Result<Result<Option<ui4::dialog::ChosenFile>, ui4::types::UiError>> {
        let r = ui3::dialog::Host::open_file(self, window, title, filter)?;
        Ok(r.map(|c| c.map(chosen_to_phase4)).map_err(error_to_phase4))
    }

    fn open_folder(
        &mut self,
        window: u64,
        title: String,
    ) -> wasmtime::Result<Result<Option<ui4::dialog::ChosenFolder>, ui4::types::UiError>> {
        let r = ui3::dialog::Host::open_folder(self, window, title)?;
        Ok(r.map(|f| {
            f.map(|f| ui4::dialog::ChosenFolder {
                name: f.name,
                token: f.token,
            })
        })
        .map_err(error_to_phase4))
    }

    fn save_file(
        &mut self,
        _window: u64,
        title: String,
        suggested: String,
        filter: String,
    ) -> wasmtime::Result<Result<Option<ui4::dialog::ChosenFile>, ui4::types::UiError>> {
        match self.ask_where_to_save(&title, &suggested, &filter) {
            Ok(Some((name, token))) => Ok(Ok(Some(ui4::dialog::ChosenFile { name, token }))),
            Ok(None) => Ok(Ok(None)),
            Err(err) => Ok(Err(error_to_phase4(err))),
        }
    }
}

fn chosen_to_phase4(c: ui3::dialog::ChosenFile) -> ui4::dialog::ChosenFile {
    ui4::dialog::ChosenFile {
        name: c.name,
        token: c.token,
    }
}

/// Translate a Phase 4 node and run it through the shared validator.
fn node_from_phase4(
    node: ui4::types::WidgetNode,
) -> Result<krate_adapter_common::ui::WidgetNode, ui3::types::UiError> {
    widget_node_from_parts(
        node.id,
        node.parent,
        kind_from_phase4(node.kind),
        node.label,
        node.role,
        WidgetStyle {
            width: node.style.width,
            height: node.style.height,
            grow: node.style.grow,
            padding: node.style.padding,
            text: node.style.text.map(text_style_from_phase4),
            place: placement_from_phase4(node.style.place),
            r#box: node.style.box_.map(box_style_from_phase4),
        },
        node.checked,
        node.value,
        node.selected,
        node.text_cursor.map(|c| (c.cursor, c.anchor)),
    )
}

/// The one place the two phases actually differ.
///
/// Every case before `overlay` maps to the kind it has always mapped to,
/// because a Phase 3 app that said `button` must still mean button, and the
/// two enums must stay in step for every case they share.
fn kind_from_phase4(kind: ui4::types::WidgetKind) -> WidgetKind {
    match kind {
        ui4::types::WidgetKind::Stack => WidgetKind::Stack,
        ui4::types::WidgetKind::Grid => WidgetKind::Grid,
        ui4::types::WidgetKind::Scroll => WidgetKind::Scroll,
        ui4::types::WidgetKind::Tabs => WidgetKind::Tabs,
        ui4::types::WidgetKind::Button => WidgetKind::Button,
        ui4::types::WidgetKind::Checkbox => WidgetKind::Checkbox,
        ui4::types::WidgetKind::Radio => WidgetKind::Radio,
        ui4::types::WidgetKind::Switch => WidgetKind::Switch,
        ui4::types::WidgetKind::Slider => WidgetKind::Slider,
        ui4::types::WidgetKind::Progress => WidgetKind::Progress,
        ui4::types::WidgetKind::Text => WidgetKind::Text,
        ui4::types::WidgetKind::TextField => WidgetKind::TextField,
        ui4::types::WidgetKind::TextArea => WidgetKind::TextArea,
        ui4::types::WidgetKind::ListView => WidgetKind::ListView,
        ui4::types::WidgetKind::TreeView => WidgetKind::TreeView,
        ui4::types::WidgetKind::Image => WidgetKind::Image,
        ui4::types::WidgetKind::Canvas => WidgetKind::Canvas,
        // The case the phase exists for.
        ui4::types::WidgetKind::Overlay => WidgetKind::Overlay,
    }
}

/// How a Phase 4 app asked for its label to be inked.
///
/// Not sanitized here: the placement builder does that for every host at
/// once, so the bounds live in one place rather than in each phase.
fn text_style_from_phase4(style: ui4::types::TextStyle) -> TextStyle {
    TextStyle {
        color: style.color.map(color_from_phase4),
        outline: style.outline.map(color_from_phase4),
        outline_width: style.outline_width,
        size: style.size,
        bold: style.bold,
    }
}

/// How a Phase 4 app asked for its box to be painted.
///
/// Not sanitized here, for the same reason `text_style_from_phase4` is not:
/// the placement builder bounds it once for every host.
fn box_style_from_phase4(style: ui4::types::BoxStyle) -> BoxStyle {
    BoxStyle {
        background: style.background.map(color_from_phase4),
        border: style.border.map(color_from_phase4),
        border_width: style.border_width,
        corner_radius: style.corner_radius,
    }
}

/// Where a Phase 4 widget asked to sit.
///
/// `None` is the default, which is the corner every widget started from
/// before placement existed -- so an app that says nothing is unchanged.
fn placement_from_phase4(place: Option<ui4::types::Placement>) -> Placement {
    match place {
        None | Some(ui4::types::Placement::Default) => Placement::Default,
        Some(ui4::types::Placement::TopLeft) => Placement::TopLeft,
        Some(ui4::types::Placement::TopCentre) => Placement::TopCentre,
        Some(ui4::types::Placement::TopRight) => Placement::TopRight,
        Some(ui4::types::Placement::CentreLeft) => Placement::CentreLeft,
        Some(ui4::types::Placement::Centre) => Placement::Centre,
        Some(ui4::types::Placement::CentreRight) => Placement::CentreRight,
        Some(ui4::types::Placement::BottomLeft) => Placement::BottomLeft,
        Some(ui4::types::Placement::BottomCentre) => Placement::BottomCentre,
        Some(ui4::types::Placement::BottomRight) => Placement::BottomRight,
    }
}

fn color_from_phase4(color: ui4::types::Color) -> Color {
    Color {
        r: color.r,
        g: color.g,
        b: color.b,
        a: color.a,
    }
}

/// The same error, in Phase 4's copy of the type.
///
/// The two are structurally identical and are distinct Rust types only
/// because they come from two generated modules.
pub(crate) fn error_to_phase4(err: ui3::types::UiError) -> ui4::types::UiError {
    match err {
        ui3::types::UiError::PermissionDenied => ui4::types::UiError::PermissionDenied,
        ui3::types::UiError::InvalidWindow => ui4::types::UiError::InvalidWindow,
        ui3::types::UiError::InvalidWidget => ui4::types::UiError::InvalidWidget,
        ui3::types::UiError::Unsupported(message) => ui4::types::UiError::Unsupported(message),
        ui3::types::UiError::Platform(message) => ui4::types::UiError::Platform(message),
    }
}

/// Phase 4's events, which differ from Phase 3's in exactly two cases.
///
/// Everything else delegates to the Phase 3 implementation and is translated
/// case for case, because the two variants are otherwise identical and the
/// Phase 3 one carries real logic -- the usability driver, interrupt
/// handling, the headless budget -- that must not be duplicated.
///
/// The two new cases are `file-dropped` and `file-hovering`. They are drained
/// FIRST, so a drop is answered before a backlog of pointer moves (K-175).
/// The mouse as a game asks about it (IC-909). Phase 4 only: Phase 3 is
/// frozen, and an interface it never had cannot break an app built for it.
impl ui4::pointer::Host for Phase3GuiHost {
    fn position(&mut self, window: u64) -> wasmtime::Result<Option<(f32, f32)>> {
        // In the app's own coordinates, as its pointer events are (K-1176).
        Ok(self
            .pointer_state(window)
            .position
            .map(|(x, y)| self.point_to_design(x, y)))
    }

    fn button_held(&mut self, button: ui4::types::PointerButton) -> wasmtime::Result<bool> {
        use krate_adapter_common::ui::PointerButton as P;
        let button = match button {
            ui4::types::PointerButton::Primary => P::Primary,
            ui4::types::PointerButton::Secondary => P::Secondary,
            ui4::types::PointerButton::Middle => P::Middle,
            ui4::types::PointerButton::Other => P::Other,
        };
        Ok(self.pointer_button_held(button))
    }

    fn take_motion(&mut self, window: u64) -> wasmtime::Result<(f32, f32)> {
        Ok(self.pointer_take_motion(window))
    }

    fn capture(&mut self, window: u64) -> wasmtime::Result<Result<(), ui4::types::UiError>> {
        Ok(self.pointer_capture(window, true).map_err(error_to_phase4))
    }

    fn release(&mut self, window: u64) -> wasmtime::Result<()> {
        let _ = self.pointer_capture(window, false);
        Ok(())
    }

    fn captured(&mut self, window: u64) -> wasmtime::Result<bool> {
        Ok(self.pointer_captured(window))
    }
}

/// The fingers on a touch screen. Phase 4 only.
impl ui4::touch::Host for Phase3GuiHost {
    fn touches(&mut self, window: u64) -> wasmtime::Result<Vec<ui4::touch::TouchPoint>> {
        Ok(self
            .touch_points(window)
            .into_iter()
            .map(|(id, x, y)| {
                // In the app's own coordinates, as its pointer events are.
                let (x, y) = self.point_to_design(x, y);
                ui4::touch::TouchPoint { id, x, y }
            })
            .collect())
    }
}

/// Pictures a canvas keeps and draws by handle. Phase 4 only.
impl crate::phase4_gui_bindings::krate::gfx::canvas_images::Host for Phase3GuiHost {
    fn keep(
        &mut self,
        canvas: u64,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    ) -> wasmtime::Result<Result<u64, gfx3::types::GfxError>> {
        Ok(self.keep_image(canvas, width, height, rgba))
    }

    fn draw(
        &mut self,
        canvas: u64,
        image: u64,
        area: gfx3::types::Rect,
    ) -> wasmtime::Result<Result<(), gfx3::types::GfxError>> {
        Ok(self.draw_kept_image(canvas, image, area, None))
    }

    fn draw_round(
        &mut self,
        canvas: u64,
        image: u64,
        area: gfx3::types::Rect,
        radii: gfx3::types::CornerRadii,
    ) -> wasmtime::Result<Result<(), gfx3::types::GfxError>> {
        Ok(self.draw_kept_image(canvas, image, area, Some(radii)))
    }

    fn forget(&mut self, canvas: u64, image: u64) -> wasmtime::Result<()> {
        self.forget_image(canvas, image);
        Ok(())
    }
}

/// Videos played in the app's own layout (IC-922). Phase 4 only. A bundled
/// clip needs no capability, as a bundled picture needs none; a stream needs
/// `net.connect` to its host, judged as a fetch is.
impl crate::phase4_gui_bindings::krate::gfx::video::Host for Phase3GuiHost {
    fn open(&mut self, source: String) -> wasmtime::Result<Result<u64, gfx3::types::GfxError>> {
        use gfx3::types::GfxError;
        let source = if source.starts_with("https://") || source.starts_with("http://") {
            if !self.may_reach(&source) {
                return Ok(Err(GfxError::PermissionDenied));
            }
            crate::video::Source::Url(source)
        } else {
            let Some(root) = self.asset_root.as_deref() else {
                return Ok(Err(GfxError::Unsupported(
                    "the app does not carry bundled assets".to_string(),
                )));
            };
            let path = match crate::phase2_host::bundled_resource_path(root, &source) {
                Ok(path) if path.is_file() => path,
                Ok(_) => {
                    return Ok(Err(GfxError::Unsupported(format!(
                        "there is no {source} in the app's assets"
                    ))))
                }
                Err(_) => {
                    return Ok(Err(GfxError::Unsupported(format!(
                        "{source} is not a path inside the app's assets"
                    ))))
                }
            };
            crate::video::Source::File(path)
        };
        Ok(match crate::video::open(source) {
            Ok(video) => {
                self.videos.borrow_mut().add(video);
                Ok(video)
            }
            Err(problem) => Err(GfxError::Platform(problem)),
        })
    }

    fn play(&mut self, video: u64) -> wasmtime::Result<Result<(), gfx3::types::GfxError>> {
        Ok(self.control_video(video, crate::video::Control::Play))
    }

    fn pause(&mut self, video: u64) -> wasmtime::Result<Result<(), gfx3::types::GfxError>> {
        Ok(self.control_video(video, crate::video::Control::Pause))
    }

    fn seek(
        &mut self,
        video: u64,
        millis: u64,
    ) -> wasmtime::Result<Result<(), gfx3::types::GfxError>> {
        Ok(self.control_video(video, crate::video::Control::Seek(millis)))
    }

    fn set_looping(
        &mut self,
        video: u64,
        looping: bool,
    ) -> wasmtime::Result<Result<(), gfx3::types::GfxError>> {
        Ok(self.control_video(video, crate::video::Control::Looping(looping)))
    }

    fn set_muted(
        &mut self,
        video: u64,
        muted: bool,
    ) -> wasmtime::Result<Result<(), gfx3::types::GfxError>> {
        Ok(self.control_video(video, crate::video::Control::Muted(muted)))
    }

    fn state(
        &mut self,
        video: u64,
    ) -> wasmtime::Result<
        Result<crate::phase4_gui_bindings::krate::gfx::video::VideoState, gfx3::types::GfxError>,
    > {
        if !self.videos.borrow().owns(video) {
            return Ok(Err(gfx3::types::GfxError::InvalidTarget));
        }
        Ok(self
            .video_state_now(video)
            .map(
                |state| crate::phase4_gui_bindings::krate::gfx::video::VideoState {
                    position_millis: state.position_millis,
                    duration_millis: state.duration_millis,
                    width: state.width,
                    height: state.height,
                    ready: state.ready,
                    playing: state.playing,
                    ended: state.ended,
                    problem: state.problem,
                },
            )
            .map_err(gfx3::types::GfxError::Platform))
    }

    fn draw(
        &mut self,
        canvas: u64,
        video: u64,
        area: gfx3::types::Rect,
    ) -> wasmtime::Result<Result<(), gfx3::types::GfxError>> {
        Ok(self.draw_video(canvas, video, area, None))
    }

    fn draw_round(
        &mut self,
        canvas: u64,
        video: u64,
        area: gfx3::types::Rect,
        radii: gfx3::types::CornerRadii,
    ) -> wasmtime::Result<Result<(), gfx3::types::GfxError>> {
        Ok(self.draw_video(canvas, video, area, Some(radii)))
    }

    fn close(&mut self, video: u64) -> wasmtime::Result<()> {
        if self.videos.borrow_mut().remove(video) {
            crate::video::close(video);
            self.video_frames.borrow_mut().remove(&video);
        }
        Ok(())
    }
}

impl Phase3GuiHost {
    fn control_video(
        &self,
        video: u64,
        control: crate::video::Control,
    ) -> Result<(), gfx3::types::GfxError> {
        if !self.videos.borrow().owns(video) {
            return Err(gfx3::types::GfxError::InvalidTarget);
        }
        // What was read before the change is no longer true.
        self.video_states.borrow_mut().remove(&video);
        crate::video::control(video, control).map_err(gfx3::types::GfxError::Platform)
    }

    /// A video's state, read from the platform at most once a frame: an
    /// app asks on every finger movement, and each ask waits for the
    /// phone's main thread, so a scrubber fell behind the finger.
    fn video_state_now(&self, video: u64) -> Result<crate::video::State, String> {
        const FRAME: std::time::Duration = std::time::Duration::from_millis(16);
        if let Some((at, state)) = self.video_states.borrow().get(&video) {
            if at.elapsed() < FRAME {
                return Ok(state.clone());
            }
        }
        let state = crate::video::state(video)?;
        self.video_states
            .borrow_mut()
            .insert(video, (std::time::Instant::now(), state.clone()));
        Ok(state)
    }

    /// Draw a video's newest frame, or the one it showed last when no newer
    /// one has arrived; nothing before the first.
    fn draw_video(
        &self,
        canvas: u64,
        video: u64,
        area: gfx3::types::Rect,
        radii: Option<gfx3::types::CornerRadii>,
    ) -> Result<(), gfx3::types::GfxError> {
        if !self.videos.borrow().owns(video) {
            return Err(gfx3::types::GfxError::InvalidTarget);
        }
        if !self.canvases_contains(canvas) {
            return Err(gfx3::types::GfxError::InvalidTarget);
        }
        match crate::video::frame(video) {
            Ok(Some(frame)) => {
                self.video_frames
                    .borrow_mut()
                    .insert(video, std::sync::Arc::new(frame));
            }
            Ok(None) => {}
            Err(problem) => return Err(gfx3::types::GfxError::Platform(problem)),
        }
        let Some(frame) = self.video_frames.borrow().get(&video).cloned() else {
            return Ok(());
        };
        self.draw_image(canvas, frame, area, radii)
    }
}

/// The phone tapping back (IC-917). Phase 4 only. No capability: a tap is
/// felt by the person holding the phone and reaches nothing else, as on the
/// phones themselves.
impl ui4::haptics::Host for Phase3GuiHost {
    fn available(&mut self) -> wasmtime::Result<bool> {
        Ok(self.haptics_available())
    }

    fn play(&mut self, feel: ui4::haptics::Feel) -> wasmtime::Result<()> {
        use krate_adapter_common::ui::HapticFeel;
        use ui4::haptics::Feel;
        let feel = match feel {
            Feel::Light => HapticFeel::Light,
            Feel::Medium => HapticFeel::Medium,
            Feel::Heavy => HapticFeel::Heavy,
            Feel::Selection => HapticFeel::Selection,
            Feel::Success => HapticFeel::Success,
            Feel::Warning => HapticFeel::Warning,
            Feel::Error => HapticFeel::Error,
        };
        self.haptic(Some(feel), 1.0);
        Ok(())
    }

    fn impact(&mut self, strength: f32) -> wasmtime::Result<()> {
        let strength = if strength.is_finite() {
            strength.clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.haptic(None, strength);
        Ok(())
    }
}

/// Reminders delivered with the app closed (IC-921). Phase 4 only, behind
/// `ui.notify`.
impl ui4::reminders::Host for Phase3GuiHost {
    fn schedule(
        &mut self,
        id: String,
        title: String,
        body: String,
        after_seconds: u64,
        daily: bool,
    ) -> wasmtime::Result<Result<(), crate::phase3_gui_bindings::krate::ui::notify::NotifyError>>
    {
        use crate::desktop_host::NotifyError;
        Ok(self
            .reminder_schedule(&id, &title, &body, after_seconds, daily)
            .map_err(|err| match err {
                NotifyError::Denied => {
                    crate::phase3_gui_bindings::krate::ui::notify::NotifyError::Denied
                }
                NotifyError::InvalidContent(m) => {
                    crate::phase3_gui_bindings::krate::ui::notify::NotifyError::InvalidContent(m)
                }
                NotifyError::Unavailable(m) => {
                    crate::phase3_gui_bindings::krate::ui::notify::NotifyError::Unavailable(m)
                }
            }))
    }

    fn cancel(&mut self, id: String) -> wasmtime::Result<()> {
        self.reminder_cancel(&id);
        Ok(())
    }

    fn pending(&mut self) -> wasmtime::Result<Vec<String>> {
        Ok(self.reminder_pending())
    }
}

/// The photo library (IC-920). Phase 4 only: picking needs no capability
/// (the pick is the grant), adding is behind `photos.add`.
impl ui4::photos::Host for Phase3GuiHost {
    fn pick(
        &mut self,
        window: u64,
        limit: u32,
    ) -> wasmtime::Result<Result<Vec<ui4::types::ChosenFile>, ui4::photos::PhotosError>> {
        Ok(self
            .pick_photos(window, limit)
            .map(|picked| {
                picked
                    .into_iter()
                    .map(|(name, token)| ui4::types::ChosenFile { name, token })
                    .collect()
            })
            .map_err(photos_error))
    }

    fn save(
        &mut self,
        _window: u64,
        data: Vec<u8>,
    ) -> wasmtime::Result<Result<(), ui4::photos::PhotosError>> {
        Ok(self.save_photo(&data).map_err(photos_error))
    }
}

fn photos_error((kind, why): (u8, String)) -> ui4::photos::PhotosError {
    match kind {
        0 => ui4::photos::PhotosError::NotAllowed,
        2 => ui4::photos::PhotosError::Invalid(why),
        _ => ui4::photos::PhotosError::Unavailable(why),
    }
}

/// The system's share sheet (IC-919). Phase 4 only; no capability, the
/// person's pick in the sheet is the grant.
impl ui4::share::Host for Phase3GuiHost {
    fn share(
        &mut self,
        window: u64,
        text: Option<String>,
        url: Option<String>,
        files: Vec<ui4::share::SharedFile>,
    ) -> wasmtime::Result<Result<bool, ui4::share::ShareError>> {
        let files = files.into_iter().map(|f| (f.name, f.data)).collect();
        Ok(
            Phase3GuiHost::share(self, window, text, url, files).map_err(|(platform, why)| {
                if platform {
                    ui4::share::ShareError::Unavailable(why)
                } else {
                    ui4::share::ShareError::Invalid(why)
                }
            }),
        )
    }
}

/// Signing in to a service through the system's sheet (IC-923). Phase 4
/// only. No capability: the person signs in on the service's own page in
/// the system browser, and the app learns only the address it is sent back
/// to; trading the code for a token needs `net.connect` like any fetch.
impl ui4::sign_in::Host for Phase3GuiHost {
    fn pkce(&mut self) -> wasmtime::Result<(String, String)> {
        Ok(crate::sign_in::pkce())
    }

    fn start(
        &mut self,
        window: u64,
        authorize_url: String,
        callback_scheme: String,
    ) -> wasmtime::Result<Result<String, ui4::sign_in::SignInError>> {
        use ui4::sign_in::SignInError;
        Ok(
            match Phase3GuiHost::sign_in(self, window, &authorize_url, &callback_scheme) {
                Ok(Some(back)) => Ok(back),
                Ok(None) => Err(SignInError::Cancelled),
                Err((true, why)) => Err(SignInError::Unavailable(why)),
                Err((false, why)) => Err(SignInError::Invalid(why)),
            },
        )
    }
}

/// A web page inside the app (IC-927). Phase 4 only, behind the
/// `net.connect` grants for the hosts it shows.
impl ui4::web_page::Host for Phase3GuiHost {
    fn open(
        &mut self,
        window: u64,
        address: String,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    ) -> wasmtime::Result<Result<u64, ui4::types::UiError>> {
        Ok(self
            .open_web_page(window, &address, (x, y, width.max(0.0), height.max(0.0)))
            .map_err(error_to_phase4))
    }

    fn place(
        &mut self,
        page: u64,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    ) -> wasmtime::Result<Result<(), ui4::types::UiError>> {
        if !self.owns_web_page(page) {
            return Ok(Err(error_to_phase4(ui3::types::UiError::InvalidWidget)));
        }
        if !self.headless_run() {
            self.place_web_page(page, (x, y, width.max(0.0), height.max(0.0)));
        }
        Ok(Ok(()))
    }

    fn go(
        &mut self,
        page: u64,
        address: String,
    ) -> wasmtime::Result<Result<(), ui4::types::UiError>> {
        Ok(self.web_page_go(page, &address).map_err(error_to_phase4))
    }

    fn back(&mut self, page: u64) -> wasmtime::Result<()> {
        if self.owns_web_page(page) && !self.headless_run() {
            Phase3GuiHost::web_page_back(self, page);
        }
        Ok(())
    }

    fn state(
        &mut self,
        page: u64,
    ) -> wasmtime::Result<Result<ui4::web_page::PageState, ui4::types::UiError>> {
        Ok(self
            .web_page_state(page)
            .map(|s| ui4::web_page::PageState {
                address: s.address,
                title: s.title,
                loading: s.loading,
                problem: s.problem,
            })
            .map_err(error_to_phase4))
    }

    fn close(&mut self, page: u64) -> wasmtime::Result<()> {
        self.close_web_page(page);
        Ok(())
    }
}

/// What a screen reader says about an app that draws itself (IC-926).
/// Phase 4 only. No capability: describing your own screen reaches
/// nothing but the person's own screen reader.
impl ui4::accessibility::Host for Phase3GuiHost {
    fn describe(
        &mut self,
        window: u64,
        elements: Vec<ui4::accessibility::Element>,
    ) -> wasmtime::Result<Result<(), ui4::types::UiError>> {
        use krate_adapter_common::ui::{AccessElement, AccessRole};
        use ui4::accessibility::Role;
        let elements = elements
            .into_iter()
            .map(|e| AccessElement {
                x: e.x,
                y: e.y,
                width: e.width.max(0.0),
                height: e.height.max(0.0),
                role: match e.role {
                    Role::Button => AccessRole::Button,
                    Role::Link => AccessRole::Link,
                    Role::Heading => AccessRole::Heading,
                    Role::Text => AccessRole::Text,
                    Role::Image => AccessRole::Image,
                    Role::Toggle => AccessRole::Toggle,
                    Role::Adjustable => AccessRole::Adjustable,
                    Role::Field => AccessRole::Field,
                },
                label: e.label,
                value: e.value,
                checked: e.checked,
                enabled: e.enabled,
            })
            .collect();
        Ok(Phase3GuiHost::describe_accessibility(self, window, elements).map_err(error_to_phase4))
    }

    fn screen_reader_on(&mut self) -> wasmtime::Result<bool> {
        Ok(Phase3GuiHost::screen_reader_on(self))
    }
}

/// Dark mode, more contrast and text size (IC-925). Phase 4 only. No
/// capability: how the person wants things to look is theirs to give every
/// app, as on the phones themselves.
impl ui4::appearance::Host for Phase3GuiHost {
    fn current(&mut self) -> wasmtime::Result<ui4::appearance::Look> {
        let look = self.appearance();
        Ok(ui4::appearance::Look {
            dark: look.dark,
            high_contrast: look.high_contrast,
            text_scale: look.text_scale,
        })
    }
}

/// Face ID, Touch ID or the passcode before something private (IC-924).
/// Phase 4 only. No capability: the system asks the person itself, and the
/// app learns only whether they passed.
impl ui4::unlock::Host for Phase3GuiHost {
    fn available(&mut self) -> wasmtime::Result<bool> {
        Ok(self.unlock_available())
    }

    fn check(&mut self, reason: String) -> wasmtime::Result<ui4::unlock::UnlockAnswer> {
        use crate::unlock::Answer;
        use ui4::unlock::UnlockAnswer;
        Ok(match self.unlock_check(&reason) {
            Answer::Yes => UnlockAnswer::Yes,
            Answer::No => UnlockAnswer::No,
            Answer::Unavailable(why) => UnlockAnswer::Unavailable(why),
        })
    }
}

/// Bluetooth devices the person picks (IC-917). Phase 4 only, behind
/// `bluetooth.connect`.
impl ui4::bluetooth::Host for Phase3GuiHost {
    fn available(&mut self) -> wasmtime::Result<bool> {
        Ok(self.bluetooth_available())
    }

    fn choose(
        &mut self,
        window: u64,
        service: String,
        title: String,
    ) -> wasmtime::Result<Result<Option<ui4::bluetooth::Device>, ui4::bluetooth::BluetoothError>>
    {
        Ok(self
            .bluetooth_choose(window, &service, &title)
            .map(|picked| {
                picked.map(|d| ui4::bluetooth::Device {
                    id: d.handle,
                    name: d.name,
                })
            })
            .map_err(bt_error))
    }

    fn read(
        &mut self,
        device: u64,
        service: String,
        characteristic: String,
    ) -> wasmtime::Result<Result<Vec<u8>, ui4::bluetooth::BluetoothError>> {
        Ok(self
            .bluetooth_read(device, &service, &characteristic)
            .map_err(bt_error))
    }

    fn write(
        &mut self,
        device: u64,
        service: String,
        characteristic: String,
        data: Vec<u8>,
        with_response: bool,
    ) -> wasmtime::Result<Result<(), ui4::bluetooth::BluetoothError>> {
        Ok(self
            .bluetooth_write(device, &service, &characteristic, &data, with_response)
            .map_err(bt_error))
    }

    fn subscribe(
        &mut self,
        device: u64,
        service: String,
        characteristic: String,
    ) -> wasmtime::Result<Result<(), ui4::bluetooth::BluetoothError>> {
        Ok(self
            .bluetooth_subscribe(device, &service, &characteristic)
            .map_err(bt_error))
    }

    fn notifications(
        &mut self,
        device: u64,
    ) -> wasmtime::Result<Vec<ui4::bluetooth::Notification>> {
        Ok(self
            .bluetooth_notifications(device)
            .into_iter()
            .map(
                |(service, characteristic, data)| ui4::bluetooth::Notification {
                    service,
                    characteristic,
                    data,
                },
            )
            .collect())
    }

    fn connected(&mut self, device: u64) -> wasmtime::Result<bool> {
        Ok(self.bluetooth_connected(device))
    }

    fn disconnect(&mut self, device: u64) -> wasmtime::Result<()> {
        self.bluetooth_disconnect(device);
        Ok(())
    }
}

fn bt_error(error: krate_adapter_common::ui::BtError) -> ui4::bluetooth::BluetoothError {
    use krate_adapter_common::ui::BtError;
    use ui4::bluetooth::BluetoothError;
    match error {
        BtError::NotAllowed => BluetoothError::NotAllowed,
        BtError::Unavailable => BluetoothError::Unavailable,
        BtError::NotFound => BluetoothError::NotFound,
        BtError::Disconnected => BluetoothError::Disconnected,
        BtError::Failed(why) => BluetoothError::Failed(why),
    }
}

/// Where the phone is (IC-917). Phase 4 only, behind `location.read`.
impl ui4::location::Host for Phase3GuiHost {
    fn available(&mut self) -> wasmtime::Result<bool> {
        Ok(self.location().0)
    }

    fn current(
        &mut self,
    ) -> wasmtime::Result<Result<ui4::location::Position, ui4::location::LocationError>> {
        use krate_adapter_common::ui::LocationAnswer;
        use ui4::location::{LocationError, Position};
        Ok(match self.location().1 {
            LocationAnswer::Here(p) => Ok(Position {
                latitude: p.latitude,
                longitude: p.longitude,
                accuracy: p.accuracy,
                altitude: p.altitude,
                speed: p.speed,
                course: p.course,
                age_ms: p.age_ms,
            }),
            LocationAnswer::NotAllowed => Err(LocationError::NotAllowed),
            LocationAnswer::Unavailable => Err(LocationError::Unavailable),
            LocationAnswer::Finding => Err(LocationError::Finding),
        })
    }
}

/// How the phone is held and moves (IC-917). Phase 4 only. No capability,
/// as with the screen's own facts: which way a phone is tilted while an app
/// is on it tells the app nothing about the person beyond that.
impl ui4::motion::Host for Phase3GuiHost {
    fn available(&mut self) -> wasmtime::Result<bool> {
        Ok(self.motion().0)
    }

    fn read(&mut self) -> wasmtime::Result<Option<ui4::motion::Reading>> {
        use ui4::motion::{Reading, Vector};
        let vector = |(x, y, z): (f32, f32, f32)| Vector { x, y, z };
        Ok(self.motion().1.map(|m| Reading {
            gravity: vector(m.gravity),
            acceleration: vector(m.acceleration),
            rotation: vector(m.rotation),
            heading: m.heading,
        }))
    }
}

/// What kind of screen a window is on (IC-912). Phase 4 only.
impl ui4::screen::Host for Phase3GuiHost {
    fn facts(&mut self, window: u64) -> wasmtime::Result<ui4::screen::ScreenFacts> {
        use ui4::screen::{InputKind, Insets, ScreenFacts, SizeClass};
        let (width, height, touch, (top, right, bottom, left)) = self
            .screen_facts(window)
            .unwrap_or((0.0, 0.0, false, (0.0, 0.0, 0.0, 0.0)));
        let class = |side: f32, compact: f32, large: f32| {
            if side < compact {
                SizeClass::Compact
            } else if side >= large {
                SizeClass::Large
            } else {
                SizeClass::Medium
            }
        };
        Ok(ScreenFacts {
            width,
            height,
            width_class: class(width, 600.0, 1000.0),
            height_class: class(height, 500.0, 900.0),
            input: if touch {
                InputKind::Touch
            } else {
                InputKind::Pointer
            },
            safe: Insets {
                top,
                right,
                bottom,
                left,
            },
        })
    }
}

/// The on-screen keyboard, for apps that draw their own text fields.
/// Phase 4 only.
impl ui4::text_entry::Host for Phase3GuiHost {
    fn start(&mut self, window: u64) -> wasmtime::Result<bool> {
        Ok(self.text_entry(window, true))
    }

    fn stop(&mut self, window: u64) -> wasmtime::Result<()> {
        self.text_entry(window, false);
        Ok(())
    }
}

impl ui4::events::Host for Phase3GuiHost {
    fn poll(&mut self) -> wasmtime::Result<Option<ui4::types::Event>> {
        if let Some(notice) = self.take_drop_notice() {
            return Ok(Some(drop_notice_to_phase4(notice)));
        }
        let event = ui3::events::Host::poll(self)?;
        Ok(event.map(event_to_phase4))
    }

    fn wait(&mut self, timeout_millis: Option<u32>) -> wasmtime::Result<Option<ui4::types::Event>> {
        if let Some(notice) = self.take_drop_notice() {
            return Ok(Some(drop_notice_to_phase4(notice)));
        }
        let event = ui3::events::Host::wait(self, timeout_millis)?;
        // A drop can arrive DURING the wait, so ask again before answering
        // nothing: otherwise a dropped file waits for the next input to shake
        // it loose.
        if event.is_none() {
            if let Some(notice) = self.take_drop_notice() {
                return Ok(Some(drop_notice_to_phase4(notice)));
            }
        }
        Ok(event.map(event_to_phase4))
    }

    fn key_held(&mut self, key: String) -> wasmtime::Result<bool> {
        ui3::events::Host::key_held(self, key)
    }

    fn gamepad_connected(&mut self) -> wasmtime::Result<bool> {
        ui3::events::Host::gamepad_connected(self)
    }

    fn gamepad_held(&mut self, button: String) -> wasmtime::Result<bool> {
        ui3::events::Host::gamepad_held(self, button)
    }

    fn gamepad_axis(&mut self, axis: String) -> wasmtime::Result<f32> {
        ui3::events::Host::gamepad_axis(self, axis)
    }
}

/// A drop or hover, as the Phase 4 guest sees it.
fn drop_notice_to_phase4(notice: crate::phase3_gui_host::DropNotice) -> ui4::types::Event {
    match notice {
        crate::phase3_gui_host::DropNotice::Dropped { name, token } => {
            ui4::types::Event::FileDropped(ui4::types::ChosenFile { name, token })
        }
        crate::phase3_gui_host::DropNotice::Hovering(over) => ui4::types::Event::FileHovering(over),
    }
}

/// Every case the two phases share, translated one for one.
///
/// Exhaustive on purpose: adding a case to Phase 3 without deciding what
/// Phase 4 does with it should not compile.
fn event_to_phase4(event: ui3::types::Event) -> ui4::types::Event {
    use ui3::types::Event as E3;
    use ui4::types::Event as E4;
    match event {
        E3::CloseRequested(id) => E4::CloseRequested(id),
        E3::Resized(size) => E4::Resized(ui4::types::WindowSize {
            width: size.width,
            height: size.height,
        }),
        E3::RedrawRequested(id) => E4::RedrawRequested(id),
        E3::Pointer(p) => E4::Pointer(ui4::types::PointerEvent {
            window: p.window,
            widget: p.widget,
            x: p.x,
            y: p.y,
            button: p.button.map(pointer_button_to_phase4),
            pressed: p.pressed,
            modifiers: modifiers_to_phase4(p.modifiers),
        }),
        E3::Key(k) => E4::Key(ui4::types::KeyEvent {
            window: k.window,
            widget: k.widget,
            key: k.key,
            pressed: k.pressed,
            modifiers: modifiers_to_phase4(k.modifiers),
        }),
        E3::Wheel(w) => E4::Wheel(ui4::types::WheelEvent {
            window: w.window,
            widget: w.widget,
            x: w.x,
            y: w.y,
            dx: w.dx,
            dy: w.dy,
            modifiers: modifiers_to_phase4(w.modifiers),
        }),
        E3::TextInput(text) => E4::TextInput(text),
        E3::TextChanged(c) => E4::TextChanged(ui4::types::TextChangedEvent {
            window: c.window,
            widget: c.widget,
            text: c.text,
        }),
        E3::Action(id) => E4::Action(id),
        E3::FocusChanged(widget) => E4::FocusChanged(widget),
        E3::ThemeChanged(theme) => E4::ThemeChanged(match theme {
            ui3::types::Theme::Light => ui4::types::Theme::Light,
            ui3::types::Theme::Dark => ui4::types::Theme::Dark,
            ui3::types::Theme::Unknown => ui4::types::Theme::Unknown,
        }),
    }
}

fn pointer_button_to_phase4(button: ui3::types::PointerButton) -> ui4::types::PointerButton {
    match button {
        ui3::types::PointerButton::Primary => ui4::types::PointerButton::Primary,
        ui3::types::PointerButton::Secondary => ui4::types::PointerButton::Secondary,
        ui3::types::PointerButton::Middle => ui4::types::PointerButton::Middle,
        ui3::types::PointerButton::Other => ui4::types::PointerButton::Other,
    }
}

fn modifiers_to_phase4(m: ui3::types::Modifiers) -> ui4::types::Modifiers {
    ui4::types::Modifiers {
        shift: m.shift,
        control: m.control,
        alt: m.alt,
        meta: m.meta,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shared_case_maps_to_the_same_kind_in_both_phases() {
        // The two enums must agree on every case they share. If a future edit
        // reorders one of them, or maps a case to the wrong kind, this says so
        // -- and a silent mismatch here would mean an app's buttons arriving
        // as checkboxes, which no screenshot would explain.
        use crate::phase3_gui_host::widget_kind_from_wit;

        let pairs = [
            (ui4::types::WidgetKind::Stack, ui3::types::WidgetKind::Stack),
            (ui4::types::WidgetKind::Grid, ui3::types::WidgetKind::Grid),
            (
                ui4::types::WidgetKind::Scroll,
                ui3::types::WidgetKind::Scroll,
            ),
            (ui4::types::WidgetKind::Tabs, ui3::types::WidgetKind::Tabs),
            (
                ui4::types::WidgetKind::Button,
                ui3::types::WidgetKind::Button,
            ),
            (
                ui4::types::WidgetKind::Checkbox,
                ui3::types::WidgetKind::Checkbox,
            ),
            (ui4::types::WidgetKind::Radio, ui3::types::WidgetKind::Radio),
            (
                ui4::types::WidgetKind::Switch,
                ui3::types::WidgetKind::Switch,
            ),
            (
                ui4::types::WidgetKind::Slider,
                ui3::types::WidgetKind::Slider,
            ),
            (
                ui4::types::WidgetKind::Progress,
                ui3::types::WidgetKind::Progress,
            ),
            (ui4::types::WidgetKind::Text, ui3::types::WidgetKind::Text),
            (
                ui4::types::WidgetKind::TextField,
                ui3::types::WidgetKind::TextField,
            ),
            (
                ui4::types::WidgetKind::TextArea,
                ui3::types::WidgetKind::TextArea,
            ),
            (
                ui4::types::WidgetKind::ListView,
                ui3::types::WidgetKind::ListView,
            ),
            (
                ui4::types::WidgetKind::TreeView,
                ui3::types::WidgetKind::TreeView,
            ),
            (ui4::types::WidgetKind::Image, ui3::types::WidgetKind::Image),
            (
                ui4::types::WidgetKind::Canvas,
                ui3::types::WidgetKind::Canvas,
            ),
        ];
        for (four, three) in pairs {
            assert_eq!(
                kind_from_phase4(four),
                widget_kind_from_wit(three),
                "{four:?} and {three:?} must mean the same host kind"
            );
        }

        // And the case Phase 3 cannot express.
        assert_eq!(
            kind_from_phase4(ui4::types::WidgetKind::Overlay),
            WidgetKind::Overlay
        );
    }
}
