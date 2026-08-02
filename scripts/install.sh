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
  # /releases/latest excludes pre-releases and Krate is pre-release only, so
  # query the full list directly (newest first) and take the newest tag that
  # starts with v. Those carry the krate binaries; the notes-* bundle releases
  # do not. Querying the list avoids a guaranteed 404 on /latest.
  version="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases?per_page=30" \
    | grep '"tag_name"' | cut -d '"' -f 4 | grep '^v' | head -1 || true)"

  # The API is rate limited to 60 requests an hour per address and does not
  # care that you are only reading. Anyone behind a shared address -- an
  # office, a university, a cafe -- can hit that without having run this
  # before, and the raw failure is a 403 that explains nothing.
  #
  # The releases page itself is plain HTML and is not rate limited the same
  # way, so fall back to reading a tag out of it.
  if [ -z "$version" ]; then
    say "The GitHub API did not answer; reading the releases page instead..."
    version="$(curl -fsSL "https://github.com/${REPO}/releases" \
      | grep -o "/${REPO}/releases/tag/v[0-9][^\"]*" \
      | cut -d / -f 6 | head -1 || true)"
  fi

  if [ -z "$version" ]; then
    echo "Could not work out the latest version." >&2
    echo "" >&2
    echo "This usually means GitHub is rate limiting your address, which happens" >&2
    echo "on shared networks and clears within the hour." >&2
    echo "" >&2
    echo "To install right now, pin a version:" >&2
    echo "  KRATE_VERSION=v0.1.0-rc8 curl -fsSL https://krate.tech/install.sh | sh" >&2
    echo "" >&2
    echo "Versions are listed at https://github.com/${REPO}/releases" >&2
    exit 1
  fi
fi

# ---- is a krate already installed, and is it this version? -----------------
#
# Re-running the installer is how you update, so say plainly whether this run
# changes anything: already current, or moving from one version to another.
installed_version=""
if command -v krate >/dev/null 2>&1; then
  # `krate --version` prints e.g. "krate 0.1.0-rc3"; take the last field.
  installed_version="$(krate --version 2>/dev/null | awk '{print $NF}' || true)"
fi
if [ -n "$installed_version" ]; then
  if [ "$installed_version" = "${version#v}" ] || [ "$installed_version" = "$version" ]; then
    say "krate ${installed_version} is already installed and up to date."
    say "Re-running will reinstall the same version."
  else
    say "Updating krate ${installed_version} -> ${version}."
  fi
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
  || {
    # Naming the likely cause beats asking the person to go and check. arm64
    # Linux is absent from rc5 on purpose: it builds in a container whose
    # libclang is too old for one dependency, and holding four working
    # platforms for it helped nobody.
    if [ "$target" = "aarch64-unknown-linux-gnu" ]; then
      die "there is no ${target} binary in ${version}. That build is temporarily
missing while a toolchain problem in its build container is fixed; every other
platform is published. Build from source in the meantime:
  git clone https://github.com/incyashraj/krate && cd krate && cargo build --release -p krate-cli"
    fi
    die "download failed. Does a binary exist for ${target} in release ${version}?"
  }

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

# Newer releases carry cargo-component beside the runtime. Placing it here is
# what turns "make an app" from a multi-minute compile into something that just
# works: upstream ships no binaries for it, so otherwise every person builds it
# from source before their first app exists. Absent in older archives, which is
# why this is quiet when there is nothing to place.
tooling_path="$(find "$tmp" -type f -name 'cargo-component*' | head -1)"
if [ -n "$tooling_path" ]; then
  chmod +x "$tooling_path"
  tooling_name="$(basename "$tooling_path")"
  if [ -w "$dir" ]; then
    cp "$tooling_path" "${dir}/${tooling_name}"
  else
    sudo cp "$tooling_path" "${dir}/${tooling_name}"
  fi
  say "Installed ${tooling_name} too, so you can make apps without a long build."
fi

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

# ---- opening and making apps ------------------------------------------------

say ""
say "To update later, just run this installer again."
say ""
# End on something the person can do, not on what they cannot do yet. The old
# ending listed the build tools `krate create` needs, which reads as homework
# to someone who has just installed and has nothing to open.
say "Try it now:"
say ""
say "  krate run https://krate.tech/notes.krate"
say ""
say "That opens a real app. Krate will show you what it wants first."
