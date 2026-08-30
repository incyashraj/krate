fn main() {
    // Tauri embeds the frontend directory into the native binary. Track it
    // explicitly so a CSS or JavaScript-only change cannot leave `cargo build`
    // reporting success while the launched Studio still serves stale assets.
    println!("cargo:rerun-if-changed=ui");
    tauri_build::build();
}
