//! Generated binding checkpoint for the Phase 2 CLI world.
//!
//! This module intentionally does not instantiate host adapters yet. Its job is
//! to prove that the Phase 2 WIT world can be consumed by Wasmtime's Component
//! Model binding generator before we freeze names and finish dispatch wiring.

wasmtime::component::bindgen!({
    path: "../../wit/layer36/phase2",
    world: "cli",
    imports: {
        default: trappable,
    },
});

#[cfg(test)]
mod tests {
    use super::*;

    fn call_run_shape(cli: &Cli, store: &mut wasmtime::Store<()>) -> wasmtime::Result<i32> {
        cli.call_run(store)
    }

    #[test]
    fn generated_cli_world_exports_run_result() {
        fn assert_run_shape(run: fn(&Cli, &mut wasmtime::Store<()>) -> wasmtime::Result<i32>) {
            let _ = run;
        }

        assert_run_shape(call_run_shape);
    }

    #[test]
    fn generated_types_keep_expected_rust_names() {
        use layer36::fs::types::OpenMode;
        use layer36::net::types::HttpMethod;

        let read = OpenMode::Read;
        let get = HttpMethod::Get;

        assert!(matches!(read, OpenMode::Read));
        assert!(matches!(get, HttpMethod::Get));
    }
}
