#!/usr/bin/env sh
# Walk `krate revise` end to end as a transaction (IC-397), with a stubbed AI.
#
# `revise` needs an AI agent, so nothing in the test suite ever drove a
# whole change: the parts were unit-tested (the staged commit, the lock, the
# fork record) and the path a person takes was not. This drives it for real
# -- a real app built from a built-in template, a real build of the changed
# source, the real commit -- with a script named `claude` on PATH standing in
# for the agent, the same seam scripts/test-cold-make.sh uses.
#
#   sh scripts/test-revise-transaction.sh [workdir]
#
# Hermetic: its own HOME, no real agent, no subscription, and the changes are
# made with the network unreachable (cargo offline, the hub a dead address),
# so it also proves a change needs no network once the toolchain is there.
# Uses the binary at KRATE_BIN, defaulting to this tree's debug build.
set -eu
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WORK="${1:-${TMPDIR:-/tmp}/krate-revise-txn}"
KRATE="${KRATE_BIN:-$ROOT/target/debug/krate}"
[ -x "$KRATE" ] || {
  echo "no krate binary at $KRATE -- build one: cargo build -p krate-cli --bin krate" >&2
  exit 1
}
rm -rf "$WORK"
mkdir -p "$WORK/bin" "$WORK/home"
export HOME="$WORK/home" KRATE_NO_USAGE=1
fail() {
  echo "REVISE TRANSACTION BROKEN: $1" >&2
  exit 1
}

# ---- the stub ----------------------------------------------------------
#
# What it does is read from files at fixed paths, not from the environment:
# the agent runs under a confined home and environment (K-755), so a variable
# set here is not promised to reach it. It answers the probe, then edits the
# app's source in the directory it is run in -- the change's working copy --
# and may also touch the app file being changed, the way another program
# could while an AI works.
cat > "$WORK/bin/claude" <<STUB
#!/bin/sh
if [ "\${2:-}" = "Reply with the single word: ok" ] || [ "\${1:-}" = "--version" ]; then
  echo ok
  exit 0
fi
mode=\$(cat "$WORK/mode")
target=\$(cat "$WORK/target" 2>/dev/null || true)
case "\$mode" in
  none) ;;
  edit) sed -i.bak "s/word,count/word,times/" src/lib.rs && rm -f src/lib.rs.bak ;;
  edit-other) sed -i.bak "s/word,count/word,amount/" src/lib.rs && rm -f src/lib.rs.bak ;;
  edit-delete) sed -i.bak "s/word,count/word,times/" src/lib.rs && rm -f src/lib.rs.bak; rm -f "\$target" ;;
  edit-overwrite) sed -i.bak "s/word,count/word,times/" src/lib.rs && rm -f src/lib.rs.bak; cp "$WORK/elsewhere.krate" "\$target" ;;
esac
exit 0
STUB
chmod +x "$WORK/bin/claude"
export PATH="$WORK/bin:$PATH"
mode() { echo "$1" > "$WORK/mode"; }
field() { "$KRATE" inspect "$1" 2>/dev/null | awk -v k="$2" '$1 == k { print $2; exit }'; }
# Offline from here on, for the changes themselves.
revise() {
  echo "$1" > "$WORK/target"
  CARGO_NET_OFFLINE=true KRATE_HUB_URL=http://127.0.0.1:1 \
    "$KRATE" revise --agent claude "$1" "call the count column times" >"$WORK/revise.log" 2>&1
}

echo "walking krate revise with a stubbed agent, in $WORK"

# ---- 0. an app with its source, made with no AI ------------------------
"$KRATE" create --kind word-frequency --name wf --yes --no-install \
  --output "$WORK/parent.krate" "count the words in a file" >"$WORK/create.log" 2>&1 \
  || fail "the template app did not build: $(tail -5 "$WORK/create.log")"
[ "$(field "$WORK/parent.krate" signature)" = "present" ] \
  || fail "a fresh home's first app is not signed with its maker key (IC-983)"
parent_archive="$(field "$WORK/parent.krate" archive)"
echo "0. built a signed app with its source"

# ---- 1. a change that changes nothing ----------------------------------
cp "$WORK/parent.krate" "$WORK/one.krate"
mode none
if revise "$WORK/one.krate"; then fail "a change that changed nothing reported success"; fi
grep -q "without changing the app" "$WORK/revise.log" || fail "an unchanged request was not named: $(tail -3 "$WORK/revise.log")"
cmp -s "$WORK/one.krate" "$WORK/parent.krate" || fail "an unchanged request still replaced the app"
ls "$WORK"/one.signed*.krate >/dev/null 2>&1 && fail "an unchanged request left a copy of the app beside it (K-1289)"
echo "1. an unchanged request leaves the app, and nothing beside it"

# ---- 2. the person's own app, changed ----------------------------------
mode edit
revise "$WORK/one.krate" || fail "a real change failed: $(tail -5 "$WORK/revise.log")"
cmp -s "$WORK/one.krate" "$WORK/parent.krate" && fail "the change did not reach the app"
grep -q "unsigned" "$WORK/revise.log" && fail "the person's own app was called an unsigned fork: $(grep unsigned "$WORK/revise.log") (K-1289)"
ls "$WORK"/one.signed*.krate >/dev/null 2>&1 && fail "the person's own app was copied beside itself (K-1289)"
[ "$(field "$WORK/one.krate" signature)" = "present" ] || fail "the changed app is not signed"
"$KRATE" inspect "$WORK/one.krate" 2>/dev/null | grep -q "derived-from" || fail "the change records no parent (IC-397, test 494)"
echo "2. the person's own app changes in place, signed, with its parent named, offline"

# ---- 3. the same change twice is the same app --------------------------
cp "$WORK/parent.krate" "$WORK/two.krate"
revise "$WORK/two.krate" || fail "the second identical change failed"
[ "$(field "$WORK/one.krate" execution)" = "$(field "$WORK/two.krate" execution)" ] \
  || fail "the same change to the same source built a different app: $(field "$WORK/one.krate" execution) vs $(field "$WORK/two.krate" execution)"
echo "3. the same change to the same app rebuilds to the same execution identity"

# ---- 4. the app deleted while the change was made ----------------------
cp "$WORK/parent.krate" "$WORK/three.krate"
mode edit-delete
# With nothing left to replace, the result is judged as a new app would be,
# and a one-line change judged as a whole request can read "built, but part
# may be missing" (exit 6). Either way the app must be written.
code=0; revise "$WORK/three.krate" || code=$?
[ "$code" = 0 ] || [ "$code" = 6 ] || fail "a change whose app was deleted meanwhile failed ($code): $(tail -3 "$WORK/revise.log")"
[ -f "$WORK/three.krate" ] || fail "the changed app was not written"
"$KRATE" inspect "$WORK/three.krate" 2>/dev/null | grep -q "derived-from" || fail "a change of a deleted app forgot its parent"
echo "4. an app deleted mid-change gets the changed app, which still names its parent"

# ---- 5. the app rewritten by something else while the change was made --
# What a sync tool or a second editor writes is a real app, changed some
# other way.
cp "$WORK/parent.krate" "$WORK/elsewhere.krate"
mode edit-other
revise "$WORK/elsewhere.krate" || fail "could not make the other program's version: $(tail -3 "$WORK/revise.log")"
cp "$WORK/parent.krate" "$WORK/four.krate"
mode edit-overwrite
if revise "$WORK/four.krate"; then fail "a change replaced an app something else rewrote meanwhile (K-1288)"; fi
grep -q "changed while this change was being made" "$WORK/revise.log" || fail "the reason was not given: $(tail -3 "$WORK/revise.log")"
cmp -s "$WORK/four.krate" "$WORK/elsewhere.krate" || fail "the other program's file was not left as it wrote it"
[ -f "$WORK/four.changed.krate" ] || fail "the change was lost instead of kept beside the app"
[ "$(field "$WORK/four.changed.krate" signature)" = "present" ] || fail "the kept change is not the finished, signed app"
echo "5. an app rewritten mid-change stands; the change is kept beside it"

echo ""
echo "krate revise is a transaction: unchanged, own app, rebuild, deleted, rewritten -- each as it should be"
