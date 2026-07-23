#!/usr/bin/env sh
# Krate installer.
#
#   curl -fsSL https://raw.githubusercontent.com/incyashraj/krate/main/scripts/install.sh | sh
#
# Downloads the krate binary for this machine from the latest GitHub release,
# verifies its checksum, and installs it to a directory on PATH. Set KRATE_VERSION
# to pin a release, or KRATE_INSTALL_DIR to choose where it lands.

set -eu

REPO="incyashraj/krate"
BINARY="krate"

say() { printf '%s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

need() {
  command -v "$1" >/dev/null 2>&1 || die "this installer needs '$1' but it was not found"
}
need curl
need tar
need uname

# ---- work out which build this machine wants -------------------------------

os="$(uname -s)"
arch="$(uname -m)"

case "$os" in
  Darwin) os_part="apple-darwin" ;;
  Linux)  os_part="unknown-linux-gnu" ;;
  *) die "unsupported operating system: $os (install from source instead)" ;;
esac

case "$arch" in
  x86_64|amd64)   arch_part="x86_64" ;;
  arm64|aarch64)  arch_part="aarch64" ;;
  *) die "unsupported architecture: $arch (install from source instead)" ;;
esac

# Only aarch64 macOS and both Linux arches ship binaries today; x86_64 macOS
# and Windows-from-sh are not covered here.
target="${arch_part}-${os_part}"

# ---- work out which version to fetch ---------------------------------------

version="${KRATE_VERSION:-}"
if [ -z "$version" ]; then
  say "Finding the latest release..."
  # /releases/latest excludes pre-releases, and Krate is pre-release only for
  # now, so fall back to the newest entry in the full release list (which the
  # API returns newest-first) and take its tag.
  version="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
    | grep '"tag_name"' | head -1 | cut -d '"' -f 4 || true)"
  if [ -z "$version" ]; then
    # Krate is pre-release only for now, so /latest is empty. Pick the newest
    # release whose tag starts with v: those carry the krate binaries, unlike
    # the notes-* bundle releases.
    version="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases?per_page=30" \
      | grep '"tag_name"' | cut -d '"' -f 4 | grep '^v' | head -1 || true)"
  fi
  [ -n "$version" ] || die "could not determine a release; set KRATE_VERSION to pin one"
fi

# The release tag keeps its leading v (v0.1.0-rc2) but the packaging script
# strips it from the archive name (krate-0.1.0-rc2-...), so match that.
archive_version="${version#v}"
archive="krate-${archive_version}-${target}.tar.gz"
base="https://github.com/${REPO}/releases/download/${version}"

# ---- download, verify, install ---------------------------------------------

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

say "Downloading ${archive}..."
curl -fSL "${base}/${archive}" -o "${tmp}/${archive}" \
  || die "download failed. Does a binary exist for ${target} in release ${version}?"

# Checksums are best effort: verify when SHA256SUMS is published, warn if not,
# never install a file that fails a check that did run.
if curl -fsSL "${base}/SHA256SUMS" -o "${tmp}/SHA256SUMS" 2>/dev/null; then
  # The sums file lists paths (dist/.../krate-...tar.gz), so match the archive
  # basename anywhere on the line rather than anchoring to the whole field.
  expected="$(grep -E "[/[:space:]]${archive}\$" "${tmp}/SHA256SUMS" \
    | head -1 | cut -d ' ' -f 1)"
  if [ -z "$expected" ]; then
    # The file exists but has no entry for our archive: something is wrong with
    # the release, so fail rather than install an unverified binary quietly.
    die "SHA256SUMS has no entry for ${archive}; refusing to install unverified"
  fi
  if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "${tmp}/${archive}" | cut -d ' ' -f 1)"
  elif command -v shasum >/dev/null 2>&1; then
    actual="$(shasum -a 256 "${tmp}/${archive}" | cut -d ' ' -f 1)"
  else
    actual=""
  fi
  if [ -n "$actual" ]; then
    [ "$actual" = "$expected" ] || die "checksum mismatch for ${archive}; refusing to install"
    say "Checksum verified."
  else
    say "Note: no sha256 tool found, skipping checksum verification."
  fi
else
  say "Note: no SHA256SUMS published for this release, skipping checksum."
fi

tar -xzf "${tmp}/${archive}" -C "$tmp"
binary_path="$(find "$tmp" -type f -name "$BINARY" | head -1)"
[ -n "$binary_path" ] || die "the archive did not contain a '${BINARY}' binary"
chmod +x "$binary_path"

# ---- choose a destination and place it -------------------------------------

dir="${KRATE_INSTALL_DIR:-}"
if [ -z "$dir" ]; then
  if [ -w "/usr/local/bin" ] 2>/dev/null; then
    dir="/usr/local/bin"
  else
    dir="${HOME}/.local/bin"
  fi
fi
mkdir -p "$dir"

if [ -w "$dir" ]; then
  cp "$binary_path" "${dir}/${BINARY}"
else
  say "Installing to ${dir} needs elevated permission..."
  sudo cp "$binary_path" "${dir}/${BINARY}"
fi

say ""
say "Installed ${BINARY} ${version} to ${dir}/${BINARY}"

# ---- tell them if it is not on PATH ----------------------------------------

case ":${PATH}:" in
  *":${dir}:"*)
    say "Run it:  krate --version"
    ;;
  *)
    say ""
    say "${dir} is not on your PATH. Add it:"
    say "  export PATH=\"${dir}:\$PATH\""
    say "Then:  krate --version"
    ;;
esac
