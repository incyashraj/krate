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
