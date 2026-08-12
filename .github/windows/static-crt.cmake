# Build C and C++ dependencies against the STATIC MSVC runtime on Windows.
#
# Why this file exists (K-071).
#
# `.cargo/config.toml` forces `-C target-feature=+crt-static` on both Windows
# targets, and it has to: without it, krate.exe imports vcruntime140.dll and
# msvcp140.dll from the Visual C++ Redistributable, which Windows 11 does not
# ship. On a clean machine the loader then refuses to start the process and it
# exits 0xC0000135 before `main` runs -- no window, no error, not even from
# `--version`. That was K-037, found on a fresh Windows 11 Pro install.
#
# So Rust links `libcmt` (static). But `whisper-rs-sys` builds whisper.cpp
# through CMake and never sets the runtime library, so CMake takes its default
# and compiles the C++ against the DYNAMIC runtime. The two halves then
# disagree, and the link fails with twenty-odd unresolved `__imp_*` UCRT
# symbols -- `__imp_fgetc`, `__imp_fputs`, `__imp_fmaxf` and friends -- next to
# `LNK4098: defaultlib 'MSVCRT' conflicts with use of other libs`.
#
# `__imp_` is the giveaway: those are import-library symbols, which only exist
# when something expects the DLL runtime. Nothing was missing from the image;
# the two sides were simply built against different runtimes.
#
# The fix is one line: tell CMake to use the static runtime too, matching what
# Rust already does. `MultiThreaded` is the static release runtime, and
# `MultiThreadedDebug` its debug counterpart -- the generator expression picks
# per configuration, because whisper builds Release even inside a debug
# `cargo test`, and mixing those is its own linker error.
#
# The `cmake` crate reads CMAKE_TOOLCHAIN_FILE from the environment, so this
# reaches whisper-rs-sys without patching or vendoring the dependency.
#
# Policy CMP0091 is what makes CMAKE_MSVC_RUNTIME_LIBRARY take effect at all;
# without NEW, CMake keeps the old behaviour of baking the flag into
# CMAKE_CXX_FLAGS and silently ignores this variable.

cmake_policy(SET CMP0091 NEW)

set(CMAKE_MSVC_RUNTIME_LIBRARY
    "MultiThreaded$<$<CONFIG:Debug>:Debug>"
    CACHE STRING "Static MSVC runtime, to match Rust's +crt-static (K-071)")
