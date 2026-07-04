//! Generated bindings for the Phase 3 `gui` component world.
//!
//! The `gui` world keeps the whole Phase 2 CLI import surface and adds the
//! Phase 3 `ui`, `gfx`, and `audio` draft interfaces. The `with` mappings
//! below reuse the Phase 2 generated modules for the shared interfaces, so
//! `Phase2Host` keeps satisfying them without duplicate trait impls, and the
//! new Phase 3 traits are implemented once by `Phase3GuiHost`.

wasmtime::component::bindgen!({
    path: "../../wit/krate/phase3",
    world: "gui",
    imports: {
        default: trappable,
    },
    with: {
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
    },
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_gui_world_exports_run_result() {
        fn assert_run_shape(run: fn(&Gui, &mut wasmtime::Store<()>) -> wasmtime::Result<i32>) {
            let _ = run;
        }

        fn call_run_shape(gui: &Gui, store: &mut wasmtime::Store<()>) -> wasmtime::Result<i32> {
            gui.call_run(store)
        }

        assert_run_shape(call_run_shape);
    }

    #[test]
    fn generated_gui_types_keep_expected_rust_names() {
        use krate::ui::types::{WidgetKind, WindowSize};

        let size = WindowSize {
            width: 640,
            height: 480,
        };
        let kind = WidgetKind::Button;

        assert_eq!(size.width, 640);
        assert!(matches!(kind, WidgetKind::Button));
    }
}
