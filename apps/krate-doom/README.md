# Doom, as a Krate app

id Software's 1993 Doom, one `.krate` file (2.8 MB with the shareware
episode inside) that opens on macOS, Windows and Linux.

- `c/doom/` is [doomgeneric](https://github.com/ozkl/doomgeneric), GPL-2.0
  (`c/LICENSE.doomgeneric`). This app, as a work that includes it, is
  distributed under the GPL-2.0 too.
- `c/libc.c` and `c/include/` are the ~45 C library functions Doom calls,
  written for a Krate app: no WASI, so nothing Krate would refuse.
- `src/lib.rs` is the platform under doomgeneric: one canvas, the clock,
  held keys, and the WAD built into the code.
- `build.rs` compiles the C to WebAssembly with a clang that knows wasm32
  (Homebrew LLVM on a Mac; `KRATE_CLANG` overrides).

The WAD is the shareware DOOM1.WAD v1.9 (md5
`f0cefca49926d00903cf57551d901abe`, 4,196,020 bytes), freely distributable
unmodified -- Debian ships it as `doom-wad-shareware`. Put it at
`../krate-doom-wad/doom1.wad` (or set `DOOM_WAD`); it is not committed.

Controls: arrows or W/S to move, A/D to strafe, Space to fire, E to open,
Enter and Escape for menus, 1-7 for weapons.

    cargo component build --release
    krate pack target/wasm32-wasip1/release/krate_doom.wasm --manifest manifest.toml -o Doom.krate
    krate run Doom.krate -- bench     # 20 s of the attract demo, then the frame rate

Measured on an M4 Mac: 51.5 fps in a window (Doom itself runs at 35).
