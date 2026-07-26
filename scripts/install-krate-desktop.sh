#!/usr/bin/env sh
# Register the .krate double-click handler on Linux (the counterpart of
# scripts/make-macos-app.sh for macOS).
#
# It installs a per-user MIME type for `.krate`, a desktop launcher that opens a
# double-clicked bundle behind the consent flow (`krate run <file> --consent`),
# and the document icon. After this, double-clicking a `.krate` in a file
# manager reviews the app's permissions and runs it — the same enforcement as
# `krate run`, only the entry gesture is new.
#
# Usage: scripts/install-krate-desktop.sh [--uninstall]
#   Honors KRATE_BINARY (path to the krate binary); defaults to `krate` on PATH.
set -eu

MIME="application/x-krate"
APP_ID="dev.krate.open"
DESKTOP="${APP_ID}.desktop"
ICON_NAME="application-x-krate"

data_home="${XDG_DATA_HOME:-$HOME/.local/share}"
app_dir="$data_home/applications"
mime_dir="$data_home/mime"
icon_dir="$data_home/icons/hicolor/512x512/mimetypes"

# Resolve the krate binary: KRATE_BINARY, else the first `krate` on PATH, else
# the local build. It must be an absolute path for the .desktop Exec line.
resolve_binary() {
  if [ -n "${KRATE_BINARY:-}" ]; then printf '%s' "$KRATE_BINARY"; return; fi
  if command -v krate >/dev/null 2>&1; then command -v krate; return; fi
  for p in target/release/krate target/debug/krate; do
    [ -x "$p" ] && { (cd "$(dirname "$p")" && printf '%s/%s' "$(pwd)" "$(basename "$p")"); return; }
  done
  echo "could not find the krate binary; set KRATE_BINARY" >&2; exit 1
}

uninstall() {
  rm -f "$app_dir/$DESKTOP" "$mime_dir/packages/krate.xml" \
        "$icon_dir/$ICON_NAME.png" 2>/dev/null || true
  command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$app_dir" >/dev/null 2>&1 || true
  command -v update-mime-database >/dev/null 2>&1 && update-mime-database "$mime_dir" >/dev/null 2>&1 || true
  echo "removed the Krate .krate association"
}

case "${1:-}" in
  --uninstall) uninstall; exit 0 ;;
  "") ;;
  *) echo "unknown argument $1" >&2; exit 2 ;;
esac

BINARY="$(resolve_binary)"
[ -x "$BINARY" ] || { echo "krate binary not executable: $BINARY" >&2; exit 1; }

mkdir -p "$app_dir" "$mime_dir/packages" "$icon_dir"

# 1. Declare the .krate MIME type.
cat > "$mime_dir/packages/krate.xml" <<XML
<?xml version="1.0" encoding="UTF-8"?>
<mime-info xmlns="http://www.freedesktop.org/standards/shared-mime-info">
  <mime-type type="$MIME">
    <comment>Krate app bundle</comment>
    <glob pattern="*.krate"/>
    <icon name="$ICON_NAME"/>
  </mime-type>
</mime-info>
XML

# 2. The launcher. %f is the double-clicked file; --consent shows the
#    permission review before the app runs. Terminal=false because a
#    double-click has no controlling terminal: `krate run --consent` asks for
#    permission through a graphical dialog (zenity/kdialog) when there is no
#    terminal, and prints a clear next step if neither is installed.
cat > "$app_dir/$DESKTOP" <<DESK
[Desktop Entry]
Type=Application
Name=Krate
Comment=Open a Krate app after reviewing what it can access
Exec=$BINARY run %f --consent
Icon=$ICON_NAME
Terminal=false
NoDisplay=true
MimeType=$MIME;
Categories=Utility;
DESK
chmod +x "$app_dir/$DESKTOP"

# 3. The document icon, if the source PNG is available.
for src in \
  "${KRATE_DOC_ICON:-}" \
  "dist/icon/krate-document-icon-1024.png" \
  "docs/landing/krate-document-icon.png"; do
  if [ -n "$src" ] && [ -f "$src" ]; then
    cp "$src" "$icon_dir/$ICON_NAME.png"
    break
  fi
done

# 4. Refresh the caches and set Krate as the default handler.
command -v update-mime-database >/dev/null 2>&1 && update-mime-database "$mime_dir" >/dev/null 2>&1 || true
command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$app_dir" >/dev/null 2>&1 || true
command -v xdg-mime >/dev/null 2>&1 && xdg-mime default "$DESKTOP" "$MIME" >/dev/null 2>&1 || true
command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -f "$data_home/icons/hicolor" >/dev/null 2>&1 || true

echo "registered .krate to open with Krate (binary: $BINARY)"
echo "double-click a .krate in your file manager, or test with:"
echo "  xdg-open path/to/app.krate"
