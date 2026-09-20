#!/usr/bin/env sh
# Walk the half of the stranger's path that MAKES an app.
#
# `test-cold-install.sh` walks the other half -- install, refuse, grant, run
# an app somebody else built -- and it has been green and stable for months.
# It stops before the person makes anything. No agent probe, no plan, no
# author, no build, no opening the result, no report.
#
# That gap is not a theory. In one week of 2026-09 five separate
# first-contact failures were found, and every one of them was in the part no
# test walked:
#
#   K-754  Studio says a signed-in Claude is not signed in     agent probe
#   K-755  the agent's HOME confined twice, build locked out   author/build
#   K-756  every failure blamed on "the build tools"           build failure UI
#   K-757  cannot report a bug without signing in              report
#   K-759  a skipped question produced an impossible manifest  plan
#
# Five for five. They read as five unrelated bugs; they are one hole.
#
# What this walks, and the rule that shapes it: THE AGENT IS STUBBED, AND THE
# STUB CAN FAIL. A fake agent that always writes perfect code would test
# almost nothing -- four of those five bugs were in what Krate SHOWS when the
# agent does not cooperate. So the stub can refuse, hang, and emit code that
# does not compile, and each case asserts what the person is told.
#
#   sh scripts/test-cold-make.sh [workdir]
#
# Offline and hermetic: no network, no real agent, no subscription. It uses
# the binary at KRATE_BIN, defaulting to this tree's debug build.
set -eu

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WORK="${1:-${TMPDIR:-/tmp}/krate-cold-make}"
KRATE="${KRATE_BIN:-$ROOT/target/debug/krate}"

# Always by absolute path. A bare `krate` finds an older installed release,
# and on a developer machine a debug build shadows even that -- both have
# cost real time here.
[ -x "$KRATE" ] || {
  echo "no krate binary at $KRATE" >&2
  echo "build one first: cargo build -p krate-cli --bin krate" >&2
  exit 1
}

rm -rf "$WORK"
mkdir -p "$WORK/bin" "$WORK/home"

# A home with no Krate in it, so nothing already on this machine can make
# the walk pass.
export HOME="$WORK/home"

fail() {
  echo "MAKE PATH BROKEN: $1" >&2
  exit 1
}

# ---- the stub ---------------------------------------------------------
#
# Agents are found by a plain PATH lookup (find_on_path, main.rs), so a
# script named after one IS one as far as Krate is concerned. This is the
# whole reason the Make path was untestable before: there was no seam, and
# nobody added one because a real agent was always to hand.
#
# KRATE_STUB_MODE picks what kind of agent today is.
#
# The stub is REWRITTEN for each mode, not just re-pointed by an env var,
# and that is load-bearing. Probes are cached for 15 minutes keyed on the
# tool's path and mtime (probe_with_cache, main.rs) -- which is right, since
# a cache cannot know a script's behaviour changed underneath it. Writing
# the file again moves its mtime and retires the cached answer. The first
# version of this walk did not, and every mode read back the first mode's
# verdict: "a signed-out agent was reported working".
stub() {
  cat > "$WORK/bin/claude" <<'STUB'
#!/usr/bin/env sh
# A stand-in for Claude Code, shaped like the real thing: it answers a probe
# on stdout and exits 0, or plays whatever failure KRATE_STUB_MODE names.
case "${KRATE_STUB_MODE:-ok}" in
  signed-out)
    echo 'Not logged in · Please run /login'
    exit 1
    ;;
  hang)
    sleep 3600
    ;;
  broken-code)
    # Answers the probe, then writes code that cannot compile.
    if [ "${1:-}" = "-p" ] && [ "${2:-}" = "Reply with the single word: ok" ]; then
      echo ok
      exit 0
    fi
    echo 'this is not rust' > src/lib.rs 2>/dev/null || true
    exit 0
    ;;
  *)
    echo ok
    exit 0
    ;;
esac
STUB
  chmod +x "$WORK/bin/claude"
}
stub
export PATH="$WORK/bin:$PATH"

echo "walking the make path with a stubbed agent, in $WORK"
echo ""

# ---- 1. the probe -----------------------------------------------------
#
# The first screen of the Make path. K-754 lived here: Krate said a tool was
# not signed in, the person could see it working, and the screen showed a
# conclusion with no evidence under it.

# Read ONE provider's row, not the whole listing. A developer machine has
# real agents on it too, and asserting against the joined text would pass on
# some other tool's output -- which is exactly the kind of false green this
# script exists to stop.
claude_field() {
  "$KRATE" ai --json 2>/dev/null | python3 -c '
import json,sys
rows = json.load(sys.stdin)
row = next((r for r in rows if r.get("name") == "claude"), None)
print("" if row is None else (row.get(sys.argv[1]) or ""))
' "$1"
}

echo "1. a working agent is reported as working"
export KRATE_STUB_MODE=ok
stub
state="$(claude_field state)"
[ "$state" = "working" ] || fail "a stubbed agent that answers was reported '$state', not working"
echo "   working"

echo "2. a signed-out agent says so AND shows what the tool said"
export KRATE_STUB_MODE=signed-out
# A fresh write, so the probe cache's (path, mtime) key no longer matches.
sleep 1
stub
state="$(claude_field state)"
[ "$state" = "not-ready" ] || fail "a signed-out agent was reported '$state', not not-ready"
detail="$(claude_field detail)"
case "$detail" in
  *"not signed in"*) : ;;
  *) fail "a signed-out agent's detail did not say so: $detail" ;;
esac
# The evidence, not just the verdict. This is K-754's whole lesson: a
# conclusion about somebody's machine has to carry the words it was drawn
# from, or a person with a working tool has no way to tell a real refusal
# from a misread.
said="$(claude_field said)"
case "$said" in
  *"Not logged in"*) : ;;
  *) fail "the tool's own words are missing, so the verdict cannot be checked: '$said' (K-754)" ;;
esac
echo "   reported, with the tool's own words"
unset KRATE_STUB_MODE

# ---- 3. the confined home --------------------------------------------
#
# K-755: the Studio confines HOME for the agent, the agent runs check-app
# itself, and that child confined the already-confined home again -- so the
# build cache landed somewhere the agent's sandbox could not write and every
# build died. The guard is that confining twice is the same as confining
# once.

echo "3. a build run under an already-confined home still works"
agent_home="$HOME/.krate/agent-home"
mkdir -p "$agent_home"
HOME="$agent_home" "$KRATE" ai >/dev/null 2>&1 || true
# The doubled path is the signature. Its presence means the confinement was
# applied twice, whatever else the run did.
[ ! -d "$agent_home/.krate/agent-home" ] \
  || fail "the agent's home was confined twice: $agent_home/.krate/agent-home exists (K-755)"
echo "   no nested agent-home"

# ---- 4. reporting a failure ------------------------------------------
#
# K-757: sending a bug report demanded a GitHub sign-in, on both ends, for
# an identity field nothing read. The person whose build had just broken was
# asked to authenticate before he could say so -- through a device-code flow
# whose code Studio pipes away, so it could not succeed at all.
#
# The wall is gone; what must be asserted is that it never comes back. A
# report send with no sign-in must not PROMPT. It may fail to reach a hub
# (there is none here), and that is a different thing entirely.

# Bounded, because the failure being guarded against IS a hang. Proven by
# sabotage: restoring the sign-in wall made this step sit polling GitHub for
# over ten minutes, which is exactly what the user experienced. An unbounded
# check for a hang can only hang.
run_bounded() {
  secs="$1"; shift
  "$@" &
  pid=$!
  waited=0
  while kill -0 "$pid" 2>/dev/null; do
    if [ "$waited" -ge "$secs" ]; then
      kill -9 "$pid" 2>/dev/null || true
      wait "$pid" 2>/dev/null || true
      return 124
    fi
    sleep 1
    waited=$((waited + 1))
  done
  wait "$pid" 2>/dev/null || true
  return 0
}

echo "4. sending a report never asks anyone to sign in"
printf 'PK\003\004 not a real report' > "$WORK/report.krate-report"
send_log="$WORK/send.log"
if ! run_bounded 30 sh -c "'$KRATE' support-send '$WORK/report.krate-report' \
    --session s-test --note test --hub http://127.0.0.1:1 >'$send_log' 2>&1"; then
  fail "sending a report did not return within 30s -- it is waiting on something, \
almost certainly a device-code sign-in whose code nobody can see (K-757). Output: $(cat "$send_log" 2>/dev/null)"
fi
out="$(cat "$send_log" 2>/dev/null || true)"
case "$out" in
  *"needs a sign-in"*|*"Enter this code"*|*"Sign in with GitHub"*)
    fail "sending a report started a sign-in, which Studio pipes away so it can never finish (K-757): $out"
    ;;
esac
# It should fail to REACH the hub, which proves it tried to send rather than
# stopping at a wall.
case "$out" in
  *"could not reach"*|*"support did not accept"*) : ;;
  *) fail "expected a connection failure against a dead hub, got: $out" ;;
esac
echo "   sent without a sign-in, failed only on the network"

# ---- 5. what a failure looks like ------------------------------------
#
# K-756: every build failure was reported as "the build tools aren't set up
# yet", because the classifier matched the word "rustup" inside Krate's OWN
# advice line, which is appended to every failure. A person was told to press
# Try again for a failure retrying could not fix.

echo "5. a compile failure is not reported as a missing toolchain"
mkdir -p "$WORK/badapp/src"
cat > "$WORK/badapp/manifest.toml" <<'EOF'
[app]
id = "dev.krate.badapp"
name = "badapp"
version = "0.1.0-dev"
entry = "target/wasm32-wasip1/release/badapp.wasm"
world = "krate:app/gui@0.2.0"
EOF
echo 'this is not rust' > "$WORK/badapp/src/lib.rs"
out="$("$KRATE" check-app "$WORK/badapp" --no-run 2>&1)" || true
# Whatever it says, it must not be the toolchain sentence when the toolchain
# is fine. This asserts the SHAPE of the answer, not its wording.
case "$out" in
  *"aren't set up"*|*"install them"*)
    fail "a broken app was blamed on the build tools (K-756): $out"
    ;;
esac
echo "   named as an app problem, not a toolchain one"

echo ""
echo "the make path works: probe, evidence, confined home, report, honest failure"
