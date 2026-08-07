#!/usr/bin/env bash
set -euo pipefail

target="${1:?usage: scripts/package.sh <target-triple> <tar.gz|zip>}"
ext="${2:?usage: scripts/package.sh <target-triple> <tar.gz|zip>}"

version="${KRATE_VERSION:-}"
if [[ -z "$version" ]]; then
  version="$(awk -F'"' '/^version = / { print $2; exit }' Cargo.toml)"
fi
version="${version#v}"

name="krate-${version}-${target}"
dist_root="dist"
package_dir="${dist_root}/${name}"
target_release="target/${target}/release"

if [[ ! -d "$target_release" ]]; then
  target_release="target/release"
fi

binary="krate"
if [[ "$target" == *windows* ]]; then
  binary="krate.exe"
fi

binary_path="${target_release}/${binary}"
if [[ ! -f "$binary_path" ]]; then
  echo "missing release binary: ${binary_path}" >&2
  exit 1
fi

rm -rf "$package_dir"
mkdir -p "$package_dir"

cp "$binary_path" "$package_dir/"
cp README.md LICENSE-MIT LICENSE-APACHE "$package_dir/"

# Windows only: the double-click handler. krate.exe is a console application,
# so Explorer opened a black console window beside every double-clicked app.
# krate-open.exe is built for the "windows" subsystem, which is what avoids
# that, and it hands the file straight to `krate run --consent`. The file
# association points at it; without it in the archive, a double-click falls
# back to the console binary and the console comes back.
if [ "${target#*windows}" != "$target" ]; then
  opener_path="target/${target}/release/krate-open.exe"
  if [ -f "$opener_path" ]; then
    cp "$opener_path" "$package_dir/"
    echo "packaged krate-open.exe (double-click handler)"
  else
    # Same reasoning: without it, double-clicking a .krate opens a console
    # window beside the app, which is the thing krate-open.exe exists to stop.
    echo "error: krate-open.exe missing from the build; a double-clicked app" >&2
    echo "       would open a console window beside it." >&2
    exit 1
  fi

  # The document icon Explorer draws for a .krate. The association script
  # looks for KrateDoc.ico beside the binary; without it in the archive there
  # is nothing to point at and every .krate gets the blank-page icon.
  if python3 scripts/make-app-icon.py "$package_dir" >/dev/null 2>&1 \
     && [ -f "$package_dir/KrateDoc.ico" ]; then
    # The generator writes both marks; only the document one belongs in the
    # archive. Krate.ico is the app icon, which nothing on Windows reads from
    # here -- shipping it is just an unexplained file next to the binary.
    rm -rf "$package_dir"/*.iconset "$package_dir"/*-1024.png "$package_dir"/*.icns \
           "$package_dir/Krate.ico"
    echo "packaged KrateDoc.ico (Explorer document icon)"
  else
    # Fail, do not warn. v0.1.0 shipped with blank-page icons because this was
    # a warning: the log said so on both Windows targets and the release went
    # out anyway. A missing icon is a defect in the archive, so the build that
    # produced it should stop.
    echo "error: could not generate KrateDoc.ico -- Windows .krate files would" >&2
    echo "       show a blank icon. Install Pillow (python3 -m pip install pillow)." >&2
    exit 1
  fi
fi

# Ship cargo-component beside the runtime when the release build produced one.
# Upstream publishes no binaries for it, so without this every person who wants
# to make an app compiles it from source first -- minutes of waiting before
# anything of theirs exists. Optional so a source build still packages fine.
tooling_binary="tooling/bin/cargo-component"
if [[ "$target" == *windows* ]]; then
  tooling_binary="tooling/bin/cargo-component.exe"
fi
if [[ -f "$tooling_binary" ]]; then
  cp "$tooling_binary" "$package_dir/"
  echo "packaged cargo-component alongside krate"
fi

case "$ext" in
  tar.gz)
    tar -C "$dist_root" -czf "${dist_root}/${name}.tar.gz" "$name"
    ;;
  zip)
    if command -v zip >/dev/null 2>&1; then
      (cd "$dist_root" && zip -qr "${name}.zip" "$name")
    elif command -v powershell >/dev/null 2>&1; then
      powershell -NoProfile -Command \
        "Compress-Archive -Path '${package_dir}' -DestinationPath '${dist_root}/${name}.zip' -Force"
    else
      echo "zip packaging requires zip or powershell" >&2
      exit 1
    fi
    ;;
  *)
    echo "unsupported package extension: ${ext}" >&2
    exit 1
    ;;
esac

echo "${dist_root}/${name}.${ext}"
