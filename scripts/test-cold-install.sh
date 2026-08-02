#!/usr/bin/env sh
# Walk the path a stranger walks, using only what is published.
#
# Nothing tested this. The installer was copied to the site and never run by
# any check, so the first command a new person types had no coverage at all --
# and it depends on things outside this repository: the release assets, the
# domain, the GitHub API, the app file served from the site. Any of those can
# break without a commit.
#
# Deliberately uses the published URLs rather than a local build. A test that
# builds from source proves the source works; it does not prove that what a
# person can actually download works.
#
#   sh scripts/test-cold-install.sh
set -eu

WORK="${1:-${TMPDIR:-/tmp}/krate-cold-install}"
SITE="${KRATE_SITE:-https://krate.tech}"

rm -rf "$WORK"
mkdir -p "$WORK"

# A home with no Krate in it, so nothing already on this machine can make the
# test pass.
export HOME="$WORK"
export KRATE_INSTALL_DIR="$WORK/bin"

fail() {
  echo "COLD PATH BROKEN: $1" >&2
  exit 1
}

echo "1. fetching the installer from $SITE"
curl -fsSL "$SITE/install.sh" -o "$WORK/install.sh" \
  || fail "could not download $SITE/install.sh"
[ -s "$WORK/install.sh" ] || fail "the installer downloaded empty"

echo "2. running it"
sh "$WORK/install.sh" > "$WORK/install.log" 2>&1 \
  || { cat "$WORK/install.log" >&2; fail "the installer exited non-zero"; }

krate="$WORK/bin/krate"
[ -x "$krate" ] || fail "no krate binary at $krate after install"

version="$("$krate" --version 2>&1)" || fail "the installed binary will not run"
echo "   installed: $version"

# The installer prints a command to try. If that command does not work, the
# first thing a new person does fails, which is the worst possible place for
# it to fail.
echo "3. the command the installer tells them to run"
grep -q "krate run" "$WORK/install.log" \
  || fail "the installer no longer suggests a command to try"

app="$SITE/notes.krate"

echo "4. running it without grants: the permission wall must stop it"
set +e
# --headless throughout: this walk captures output and compares it, so a
# window that opens and waits for somebody to close it would hang the script
# forever on any machine with a display. What is under test here is the
# permission wall and the app's answer, not the window.
denied_out="$("$krate" run --headless "$app" 2>&1)"
denied="$?"
set -e
[ "$denied" -eq 5 ] || fail "expected exit 5 without grants, got $denied"
case "$denied_out" in
  *"needs permission"*) : ;;
  *) fail "the refusal did not explain itself: $denied_out" ;;
esac
case "$denied_out" in
  *--grant*) : ;;
  *) fail "the refusal did not say how to allow it" ;;
esac
echo "   refused, in plain words, with a way forward"

echo "5. running it with grants: the app must work"
cd "$WORK"
out="$("$krate" run --headless --grant 'fs.read:notes/**' --grant 'fs.write:notes/**' "$app" 2>&1)" \
  || fail "the app failed with its grants: $out"
case "$out" in
  *note*) : ;;
  *) fail "the app ran but produced nothing recognisable: $out" ;;
esac
echo "   ran: $out"

echo ""
echo "the cold path works: install from $SITE, refuse, grant, run"
