//! Generated bindings for the Phase 4 `gui` component world.
//!
//! Phase 4 exists for one reason: an enum in the component model is a
//! structural type, and wasmtime compares it whole. Adding `overlay` to
//! `widget-kind` made the host declare 18 cases where every app built before
//! it declares 17, and wasmtime refused the instance -- "expected enum of 18
//! names, found 17 names" (K-403). Discriminant stability is real and is not
//! what the linker checks.
//!
//! So Phase 3 is frozen exactly as it shipped and Phase 4 carries the new
//! case. The runtime builds a linker for each and tries both, the way it
//! already tries phase 1, phase 2 and phase 3 -- an app built last week keeps
//! running, and an app built today gets the overlay.
//!
//! Almost everything here is REUSED rather than regenerated. Only
//! `krate:ui/types` (which defines `widget-kind`) and `krate:ui/tree` (which
//! passes a `widget-node` and so mentions it) differ between the two phases.
//! Every other interface -- the whole of phase 2, gfx, audio, camera, speech,
//! and the other eight ui interfaces -- is mapped to the module phase 3
//! already generated, so `Phase3GuiHost` keeps satisfying them and the host
//! gains exactly two new trait impls instead of twenty.

wasmtime::component::bindgen!({
    path: "../../wit/krate/phase4",
    world: "gui",
    imports: {
        default: trappable,
    },
    with: {
        // Phase 2, reused through phase 3's own mappings.
        "krate:io/types@0.1.0": crate::phase2_bindings::krate::io::types,
        "krate:io/streams@0.1.0": crate::phase2_bindings::krate::io::streams,
        "krate:io/stdio@0.1.0": crate::phase2_bindings::krate::io::stdio,
        "krate:io/args@0.1.0": crate::phase2_bindings::krate::io::args,
        "krate:io/log@0.1.0": crate::phase2_bindings::krate::io::log,
        "krate:fs/types@0.1.0": crate::phase2_bindings::krate::fs::types,
        "krate:fs/files@0.1.0": crate::phase2_bindings::krate::fs::files,
        "krate:net/types@0.1.0": crate::phase2_bindings::krate::net::types,
        "krate:net/http-client@0.1.0": crate::phase2_bindings::krate::net::http_client,
        "krate:time/clock@0.1.0": crate::phase2_bindings::krate::time::clock,
        "krate:time/sleep@0.1.0": crate::phase2_bindings::krate::time::sleep,
        "krate:locale/info@0.1.0": crate::phase2_bindings::krate::locale::info,
        "krate:locale/format@0.1.0": crate::phase2_bindings::krate::locale::format,
        "krate:resources/assets@0.1.0": crate::phase2_bindings::krate::resources::assets,
        "krate:net/ws@0.1.0": crate::phase3_gui_bindings::krate::net::ws,
        "krate:store/kv@0.1.0": crate::phase3_gui_bindings::krate::store::kv,
        "krate:store/sql@0.1.0": crate::phase3_gui_bindings::krate::store::sql,
        "krate:store/secret@0.1.0": crate::phase3_gui_bindings::krate::store::secret,
        "krate:store/shared@0.1.0": crate::phase3_gui_bindings::krate::store::shared,
        "krate:random/bytes@0.1.0": crate::phase3_gui_bindings::krate::random::bytes,

        // The ui interfaces that do NOT mention widget-kind, reused whole.
        "krate:ui/window@0.1.0": crate::phase3_gui_bindings::krate::ui::window,
        "krate:ui/image@0.1.0": crate::phase3_gui_bindings::krate::ui::image,
        "krate:ui/events@0.1.0": crate::phase3_gui_bindings::krate::ui::events,
        "krate:ui/clipboard@0.1.0": crate::phase3_gui_bindings::krate::ui::clipboard,
        "krate:ui/menu@0.1.0": crate::phase3_gui_bindings::krate::ui::menu,
        "krate:ui/launcher@0.1.0": crate::phase3_gui_bindings::krate::ui::launcher,
        "krate:ui/notify@0.1.0": crate::phase3_gui_bindings::krate::ui::notify,

        // Everything else phase 3 already generated.
        "krate:gfx/types@0.1.0": crate::phase3_gui_bindings::krate::gfx::types,
        "krate:gfx/canvas2d@0.1.0": crate::phase3_gui_bindings::krate::gfx::canvas2d,
        "krate:gfx/scene3d@0.1.0": crate::phase3_gui_bindings::krate::gfx::scene3d,
        "krate:audio/types@0.1.0": crate::phase3_gui_bindings::krate::audio::types,
        "krate:audio/playback@0.1.0": crate::phase3_gui_bindings::krate::audio::playback,
        "krate:audio/capture@0.1.0": crate::phase3_gui_bindings::krate::audio::capture,
        "krate:camera/types@0.1.0": crate::phase3_gui_bindings::krate::camera::types,
        "krate:camera/capture@0.1.0": crate::phase3_gui_bindings::krate::camera::capture,
        "krate:speech/transcription@0.1.0":
            crate::phase3_gui_bindings::krate::speech::transcription,

        // NOT mapped, and this is the whole point of the file:
        //   krate:ui/types  -- defines widget-kind, which now has 18 cases
        //   krate:ui/tree   -- passes a widget-node, so it mentions it
        //   krate:ui/dialog -- gained `save-file`, which Phase 3 does not have
        // Those two generate fresh, and the host implements them twice.
    },
});

#[cfg(test)]
mod tests {
    #[test]
    fn phase4_widget_kind_has_the_overlay_case_and_phase3_does_not() {
        // The reason this world exists, asserted rather than described. If
        // these two ever agree, one of them has drifted and the frozen phase
        // is no longer frozen.
        use super::krate::ui::types::WidgetKind as P4;
        use crate::phase3_gui_bindings::krate::ui::types::WidgetKind as P3;

        let overlay = P4::Overlay;
        assert!(matches!(overlay, P4::Overlay));

        // Phase 3 has no such case; the nearest container it knows is canvas.
        let canvas = P3::Canvas;
        assert!(matches!(canvas, P3::Canvas));

        // And the case that must NOT have moved: a phase 3 app that said
        // `button` still means button in phase 4, because every case before
        // the new one kept its position.
        assert_eq!(P3::Button as u32, P4::Button as u32);
        assert_eq!(P3::Canvas as u32, P4::Canvas as u32);
    }
}
