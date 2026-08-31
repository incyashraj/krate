//! Compile a Krate app for the browser.
//!
//! A browser build carries no compiler, so a component must arrive already
//! compiled -- for pulley, the interpreter a tab runs, and with the same
//! engine settings the tab will use. Both come from the runtime itself
//! rather than being restated here.
//!
//! ```text
//! cargo run -p krate-runtime --example precompile_for_web -- in.wasm out.cwasm
//! ```

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(input), Some(output)) = (args.next(), args.next()) else {
        eprintln!("usage: precompile_for_web <component.wasm> <out.cwasm>");
        std::process::exit(2);
    };

    let bytes = std::fs::read(&input).expect("read the component");
    // The UI mode is not a detail here. It decides whether epoch
    // interruption is compiled in, and an artifact built without it will
    // not load into an engine that has it -- wasmtime answers
    // "compilation settings are not compatible with the native host".
    //
    // A previewed app opens a window, so this must be a windowed mode,
    // NOT the headless default a bare Config::default() would give.
    let config = krate_runtime::Config {
        phase3_ui_mode: krate_runtime::phase3_ui::Phase3HostUiMode::NativePrototype,
        ..krate_runtime::Config::default()
    };
    let compiled = krate_runtime::Runtime::precompile_component_for_web(&config, &bytes)
        .expect("precompile for the web");
    std::fs::write(&output, &compiled).expect("write the artifact");
    println!(
        "{} ({} bytes) -> {} ({} bytes)",
        input,
        bytes.len(),
        output,
        compiled.len()
    );
}
