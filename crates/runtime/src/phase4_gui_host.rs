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

use krate_adapter_common::ui::{Color, TextStyle, WidgetKind, WidgetStyle};

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
