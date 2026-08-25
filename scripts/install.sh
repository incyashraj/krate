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

# Both macOS arches (Intel and Apple Silicon) and both Linux arches ship
# binaries. Windows installs through install.ps1, not this script.
target="${arch_part}-${os_part}"

# ---- work out which version to fetch ---------------------------------------

version="${KRATE_VERSION:-}"
if [ -z "$version" ]; then
  # /releases/latest is the STABLE CHANNEL. The release pipeline publishes
  # every build as a pre-release and promotes it to latest only after the
  # published assets pass verification the way a new user would experience
  # them (checksums, Gatekeeper, a real app built and run). Resolving
  # anything newer than latest would hand out an unverified build -- which
  # is exactly what ranking the whole release list used to do.
  version="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
    | grep '"tag_name"' | cut -d '"' -f 4 || true)"

  # The API is rate limited to 60 requests an hour per address and does not
  # care that you are only reading. Anyone behind a shared address -- an
  # office, a university, a cafe -- can hit that without having run this
  # before, and the raw failure is a 403 that explains nothing.
  #
  # The /releases/latest redirect carries the same answer without the API:
  # follow it and read the promoted tag off the final URL.
  if [ -z "$version" ]; then
    version="$(curl -fsSLI -o /dev/null -w '%{url_effective}' \
      "https://github.com/${REPO}/releases/latest" \
      | grep -o 'tag/v[0-9][^/]*$' | cut -d / -f 2 || true)"
  fi

  if [ -z "$version" ]; then
    echo "Could not work out the latest version." >&2
    echo "" >&2
    echo "This usually means GitHub is rate limiting your address, which happens" >&2
    echo "on shared networks and clears within the hour." >&2
    echo "" >&2
    echo "To install right now, pin a version:" >&2
    echo "  KRATE_VERSION=<tag> curl -fsSL https://krate.tech/install.sh | sh" >&2
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

# One line that says what is happening. Every check still runs -- a checksum
# mismatch still refuses loudly -- quiet is not the same as careless.
case "$target" in
  x86_64-apple-darwin)      friendly="macos-intel" ;;
  aarch64-apple-darwin)     friendly="macos-apple-silicon" ;;
  x86_64-unknown-linux-gnu) friendly="linux-x86_64" ;;
  aarch64-unknown-linux-gnu) friendly="linux-arm64" ;;
  *)                        friendly="$target" ;;
esac
say "Installing Krate ${version} (${friendly})..."
curl -fSL --progress-bar "${base}/${archive}" -o "${tmp}/${archive}" \
  || {
    # Naming the likely cause beats asking the person to go and check. arm64
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
    say "  Downloading... done (verified)"
  else
    say "  Downloading... done (no sha256 tool found, checksum skipped)"
  fi
else
  say "  Downloading... done (no SHA256SUMS published, checksum skipped)"
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

say "  Installed to ${dir}"

# ---- double-click: install Krate.app so a .krate opens from Finder ----------
#
# The CLI alone lets you `krate run app.krate` in a terminal. But the point of a
# shareable app is that someone double-clicks it, so on macOS we also fetch the
# prebuilt Krate.app and register it: the app declares the .krate document type,
# and Finder then routes a double-clicked bundle through it to the same consent
# flow `krate run` uses. Quiet and skippable -- a missing app zip or a headless
# box just means terminal-only, not a failed install.
if [ "$os" = "Darwin" ] && [ "${KRATE_NO_DESKTOP:-}" != "1" ]; then
  app_archive="krate-app-${archive_version}-${target}.zip"
  if curl -fsSL "${base}/${app_archive}" -o "${tmp}/${app_archive}" 2>/dev/null; then
    # Unzip into a temp spot, then move Krate.app into /Applications (or the
    # per-user Applications folder if that is not writable), replacing any old
    # copy so an update refreshes the handler.
    app_unzip="${tmp}/app"
    mkdir -p "$app_unzip"
    if command -v unzip >/dev/null 2>&1 && unzip -oq "${tmp}/${app_archive}" -d "$app_unzip" 2>/dev/null; then
      src_app="$(find "$app_unzip" -maxdepth 2 -type d -name 'Krate.app' | head -1)"
      if [ -n "$src_app" ]; then
        apps_dir="/Applications"
        [ -w "$apps_dir" ] 2>/dev/null || apps_dir="${HOME}/Applications"
        mkdir -p "$apps_dir"
        rm -rf "${apps_dir}/Krate.app" 2>/dev/null || true
        if cp -R "$src_app" "${apps_dir}/Krate.app" 2>/dev/null; then
          # The app's shim execs `krate open-app`, so point it at the CLI we
          # just installed and register the bundle with Launch Services so the
          # .krate association takes effect without a logout.
          if command -v /System/Library/Frameworks/CoreServices.framework/Versions/A/Frameworks/LaunchServices.framework/Versions/A/Support/lsregister >/dev/null 2>&1; then
            /System/Library/Frameworks/CoreServices.framework/Versions/A/Frameworks/LaunchServices.framework/Versions/A/Support/lsregister -f "${apps_dir}/Krate.app" >/dev/null 2>&1 || true
          fi
          say "  .krate files now open on double-click."
        fi
      fi
    fi
  fi
fi

# On Linux, register the per-user .krate handler (MIME + launcher + icon) via
# the desktop-integration script if it is reachable. A curl install does not
# have the repo, so this is best-effort: fetch the script and run it pointed at
# the binary we just installed. Skippable with KRATE_NO_DESKTOP=1.
if [ "$os" = "Linux" ] && [ "${KRATE_NO_DESKTOP:-}" != "1" ]; then
  desktop_script="${tmp}/install-krate-desktop.sh"
  if curl -fsSL "https://raw.githubusercontent.com/${REPO}/main/scripts/install-krate-desktop.sh" -o "$desktop_script" 2>/dev/null; then
    if KRATE_BINARY="${dir}/${BINARY}" sh "$desktop_script" >/dev/null 2>&1; then
      say "  .krate files now open on double-click."
    fi
  fi
fi

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
fi

# ---- tell them if it is not on PATH ----------------------------------------

case ":${PATH}:" in
  *":${dir}:"*) : ;;
  *)
    say ""
    say "${dir} is not on your PATH. Add it:"
    say "  export PATH=\"${dir}:\$PATH\""
    ;;
esac

# ---- does `krate` actually reach what was just installed? -------------------
#
# "Installed!" is a lie if a different krate sits earlier on PATH -- a dev's
# debug build, an old copy in /usr/local/bin -- because the person then runs
# the stale one, hits "built against different versions of the app
# interface", and reads the error's own advice: update Krate. Which they just
# did. Say the true thing instead: name the shadowing binary and the exact
# command that runs the new one.
resolved="$(command -v krate 2>/dev/null || true)"
if [ -n "$resolved" ] && [ "$resolved" != "${dir}/krate" ]; then
  say ""
  say "Heads up: 'krate' in this shell runs ${resolved},"
  say "not the copy just installed. Until your PATH puts ${dir} first,"
  say "use the full path:"
  say "  ${dir}/krate run app.krate"
fi

# ---- the invitation ---------------------------------------------------------

# End on the one word, in the colour that says "you are done". Someone who
# has just installed something wants to know how to start it, and `krate` is
# the answer to every question they have next. The `krate run` line stays: it
# is the command for an app somebody was sent, and the cold-install walk
# checks the installer suggests it.
say ""
say "Sent an app? Double-click the .krate file, or: krate run app.krate"
say ""
if [ -t 1 ]; then
  printf '\033[32m%s\033[0m\n' "Run 'krate' to get started!"
else
  say "Run 'krate' to get started!"
fi
