# The web spike: Krate's painter in a browser tab

Proof that a Krate app can be previewed live on a web page, which is
what the web builder needs (`Plan/Web-Builder-2026-08-31.md`).

What runs here is the real thing, not a mock: a widget tree goes through
`krate-layout` (the same layout engine the desktop uses) and is painted
by `krate-adapter-common` (the same painter), then lands on a canvas
through `putImageData`.

## Rebuild it

```
cargo build -p krate-adapter-web --target wasm32-unknown-unknown --release
wasm-bindgen --target web --out-dir evidence/web-spike \
  target/wasm32-unknown-unknown/release/krate_adapter_web.wasm
python3 -m http.server 8901 --directory evidence/web-spike
```

Then open http://localhost:8901/.

`wasm-bindgen` must match the version in Cargo.lock, or the module
refuses to load:

```
cargo install wasm-bindgen-cli --version 0.2.120
```

## What it proved

- The painter is a CPU framebuffer (`0xAARRGGBB` in a row-major
  `&mut [u32]`), which is byte-compatible with `ImageData`. No WebGPU,
  no WebGL, no shaders.
- `krate-adapter-common` and `krate-layout` compile to wasm32 untouched.
- Measured in the browser: 30,229 non-background pixels, all opaque,
  no console errors.

The generated `.js` and `.wasm` are not committed -- they are build
output, and the command above regenerates them.
