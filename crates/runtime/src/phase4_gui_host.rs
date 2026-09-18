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
fn error_to_phase4(err: ui3::types::UiError) -> ui4::types::UiError {
    match err {
        ui3::types::UiError::PermissionDenied => ui4::types::UiError::PermissionDenied,
        ui3::types::UiError::InvalidWindow => ui4::types::UiError::InvalidWindow,
        ui3::types::UiError::InvalidWidget => ui4::types::UiError::InvalidWidget,
        ui3::types::UiError::Unsupported(message) => ui4::types::UiError::Unsupported(message),
        ui3::types::UiError::Platform(message) => ui4::types::UiError::Platform(message),
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
