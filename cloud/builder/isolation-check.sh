#!/bin/sh
#
# K-1293 and K-1297 on the live builder: can a build read the service's
# secrets, another build's, or change what the next build runs?
#
# Run on the machine, as root, after a deploy:
#
#   fly ssh console --app krate-builder -C krate-isolation-check
#
# Prints one line per check and exits 1 if any fails. It reads the service's
# secret only to look for it in what a build can see; it never prints it.
set -u

fail=0
ok() { echo "ok    $*"; }
bad() { echo "FAIL  $*"; fail=1; }
note() { echo "note  $*"; }

base="${KRATE_BUILD_USER:-kratebuild}"
slots="${KRATE_BUILD_SLOTS:-0}"
if [ "$slots" -gt 0 ] 2>/dev/null; then
  users="$(i=1; while [ "$i" -le "$slots" ]; do printf '%s%s ' "$base" "$i"; i=$((i + 1)); done)"
else
  users="$base"
  note "KRATE_BUILD_SLOTS is not set: one build user, so one build at a time"
fi
as_user() { u="$1"; shift; setpriv --reuid="$u" --regid="$u" --clear-groups "$@"; }

[ "$(id -u)" = "0" ] || { echo "run this as root on the builder"; exit 2; }
seen_uids=""
for user in $users; do
  if getent passwd "$user" >/dev/null; then
    uid="$(id -u "$user")"
    case " $seen_uids " in *" $uid "*) bad "$user shares uid $uid with another build user";; *) ok "the build user $user exists (uid $uid)";; esac
    seen_uids="$seen_uids $uid"
  else
    bad "there is no $user user"
  fi
done
first="${users%% *}"

# The service: the node process running server.js.
pid=""
for p in /proc/[0-9]*; do
  if tr '\0' ' ' < "$p/cmdline" 2>/dev/null | grep -q "node /srv/src/server.js"; then pid="${p#/proc/}"; break; fi
done
if [ -z "$pid" ]; then echo "the service is not running"; exit 2; fi
svc_uid="$(awk '/^Uid:/ {print $2}' "/proc/$pid/status")"
[ "$svc_uid" = "0" ] && ok "the service (pid $pid) runs as root" || bad "the service runs as uid $svc_uid, so it cannot hand builds to $user"

# What a build must never see: the hub secret, else the model key.
secret="$(tr '\0' '\n' < "/proc/$pid/environ" | sed -n 's/^KRATE_BUILDER_SECRET=//p')"
[ -n "$secret" ] || secret="$(tr '\0' '\n' < "/proc/$pid/environ" | sed -n 's/^ANTHROPIC_API_KEY=//p')"
[ -n "$secret" ] && ok "a service secret is set, to look for" || note "no KRATE_BUILDER_SECRET or ANTHROPIC_API_KEY on the service; the leak checks below prove less"

# Every build user, reaching for the service's files and environment, and
# for what every build shares (K-1297): none may write the toolchain, the
# image's cargo home, the engine or wasi-sdk.
for user in $users; do
  for target in "/proc/$pid/environ" /proc/1/environ; do
    if as_user "$user" cat "$target" >/dev/null 2>&1; then bad "$user can read $target"; else ok "$user cannot read $target"; fi
  done
  if as_user "$user" ls /work/state >/dev/null 2>&1; then bad "$user can list /work/state (other people's apps)"; else ok "$user cannot list /work/state"; fi
  if as_user "$user" ls /work/judge >/dev/null 2>&1; then bad "$user can list /work/judge"; else ok "$user cannot list /work/judge"; fi
  if as_user "$user" sh -c 'echo x > /work/judge/probe' 2>/dev/null; then bad "$user can write /work/judge"; rm -f /work/judge/probe; else ok "$user cannot write /work/judge"; fi
  for shared in "${RUSTUP_HOME:-/usr/local/rustup}" "${CARGO_HOME:-/usr/local/cargo}" /usr/local/bin /opt/wasi-sdk; do
    w="$(as_user "$user" find "$shared" -writable -print -quit 2>/dev/null)"
    if [ -n "$w" ]; then bad "$user can write $w (shared by every build)"; else ok "$user can write nothing under $shared"; fi
  done
  for other in /work/build/*/; do
    [ -d "$other" ] || continue
    owner="$(stat -c %U "$other")"
    [ "$owner" = "$user" ] && continue
    if as_user "$user" ls "$other" >/dev/null 2>&1; then bad "$user can list $other ($owner's job)"; else ok "$user cannot list $other"; fi
  done
done
for gone in /work/.cache /work/build/.cache; do
  [ -e "$gone" ] && bad "$gone exists: the old shared cache is back" || ok "no shared cache at $gone"
done

# Children started exactly the way the server starts them (src/sandbox.js):
# two jobs at once. Job A keeps a file in its home and a secret in its
# environment and waits; job B tries to read both, and both environs of
# the service. The probe's pool has its own homes (/work/isolation-check),
# and kills only what it started, so it never touches a live build.
probe="/work/isolation-check"
rm -rf "$probe"; mkdir -p "$probe"; chmod 0755 "$probe"
fake_passwd="$probe/passwd"
for user in $users; do
  getent passwd "$user" | awk -F: -v h="$probe/$(echo "$user" | tr -dc 'a-z0-9')" 'BEGIN{OFS=":"} {$6=h; print}' >> "$fake_passwd"
done
out="$(cd /srv && KRATE_BUILD_PASSWD="$fake_passwd" node --input-type=module -e "
  import { createSandbox } from '/srv/src/sandbox.js';
  const box = createSandbox({ log: { log() {}, error() {} }, pidsOf: () => [] });
  await box.ready;
  if (!box.isolated) { console.log('NOT-ISOLATED ' + (box.refusal() || '')); process.exit(0); }
  const run = (l, script, extra) => new Promise((done) => {
    const p = l.spawn('sh', ['-c', script], { stdio: ['ignore', 'pipe', 'pipe'], extra });
    let s = ''; p.stdout.on('data', (b) => { s += b; }); p.stderr.on('data', (b) => { s += b; });
    p.on('close', () => done(s));
  });
  const a = await box.lease();
  const b = box.size > 1 ? await box.lease() : null;
  if (!b) { console.log('ONE-SLOT'); }
  const holder = a.spawn('sh', ['-c', 'echo held > \$HOME/tmp/a-secret; sleep 20'], { stdio: 'ignore', extra: { KRATE_PROBE_MARK: 'job-a-only' } });
  await new Promise((r) => setTimeout(r, 300));
  let s = 'A-UID=' + a.user.uid + '\n';
  if (b) {
    s += 'B-UID=' + b.user.uid + '\n';
    s += await run(b, 'cat /proc/$pid/environ /proc/1/environ 2>/dev/null; env; echo UID=\$(id -u); ls /work/state 2>/dev/null && echo LISTED-STATE; cat ' + a.home + '/tmp/a-secret 2>/dev/null && echo READ-A-HOME; tr \'\\\\0\' \'\\\\n\' < /proc/' + holder.pid + '/environ 2>/dev/null | grep -q job-a-only && echo READ-A-ENV; echo B-DONE');
  }
  await a.release(); if (b) await b.release();
  const fs = await import('node:fs');
  s += 'HOMES-LEFT=' + [a.home, b && b.home].filter((h) => h && fs.existsSync(h)).length + '\n';
  s += 'FREE=' + box.free() + '/' + box.size + '\n';
  process.stdout.write(s);
" 2>&1)"
rm -rf "$probe"
case "$out" in *NOT-ISOLATED*) bad "the sandbox says builds are not isolated on this machine: $(printf '%s' "$out" | sed -n 's/.*NOT-ISOLATED //p')";; esac
case "$out" in *ONE-SLOT*) note "one build user: the two-jobs-at-once checks need KRATE_BUILD_SLOTS of 2 or more";; esac
a_uid="$(printf '%s\n' "$out" | sed -n 's/^A-UID=//p')"
b_uid="$(printf '%s\n' "$out" | sed -n 's/^B-UID=//p')"
build_uid="$(printf '%s\n' "$out" | sed -n 's/^UID=//p')"
if [ -n "$b_uid" ]; then
  [ "$a_uid" != "$b_uid" ] && ok "two jobs at once run as two uids ($a_uid, $b_uid)" || bad "two jobs at once shared uid $a_uid"
  [ "$build_uid" = "$b_uid" ] && ok "a sandboxed child runs as its lease's uid $build_uid" || bad "a sandboxed child ran as uid '${build_uid}', not $b_uid"
  printf '%s\n' "$out" | grep -q 'READ-A-HOME' && bad "job B read job A's home" || ok "job B cannot read job A's home"
  printf '%s\n' "$out" | grep -q 'READ-A-ENV' && bad "job B read job A's environment" || ok "job B cannot read job A's environment"
fi
if [ -n "$secret" ] && printf '%s' "$out" | grep -qF "$secret"; then bad "a sandboxed child SAW THE SECRET"; else ok "a sandboxed child did not see the secret"; fi
printf '%s\n' "$out" | grep -q '^KRATE_BUILDER_SECRET=' && bad "KRATE_BUILDER_SECRET is in a child's environment" || ok "KRATE_BUILDER_SECRET is not in a child's environment"
printf '%s\n' "$out" | grep -q '^ANTHROPIC_API_KEY=' && bad "a keyless child got ANTHROPIC_API_KEY" || ok "a keyless child has no model key"
printf '%s\n' "$out" | grep -q 'LISTED-STATE' && bad "a sandboxed child listed /work/state" || ok "a sandboxed child cannot list /work/state"
[ "$(printf '%s\n' "$out" | sed -n 's/^HOMES-LEFT=//p')" = "0" ] && ok "a job's home is gone once it hands its user back" || bad "a job's home outlived its job"
free="$(printf '%s\n' "$out" | sed -n 's/^FREE=//p')"
[ -n "$free" ] && [ "${free%/*}" = "${free#*/}" ] && ok "every build user is back in the probe's pool ($free)" || bad "build users not back in the pool: '$free'"

# What the service says about itself.
if curl -fsS "http://127.0.0.1:${PORT:-8080}/health" | grep -q '"isolation":"build-user"'; then ok "/health says isolation: build-user"; else bad "/health does not say isolation: build-user"; fi
health="$(curl -fsS "http://127.0.0.1:${PORT:-8080}/health" 2>/dev/null)"
case "$health" in *'"slots":{"size":'*) ok "/health reports its pool: $(printf '%s' "$health" | sed -n 's/.*"slots":\({[^}]*}\).*/\1/p')";; *) bad "/health has no build-user pool";; esac

# Fly's own socket inside the machine: reported, not judged. If a build can
# reach it, what it can ask for is the next thing to check.
if [ -e /.fly/api ]; then
  if as_user "$first" test -r /.fly/api -a -w /.fly/api; then note "$first can open /.fly/api (Fly's machine API socket): check what it answers"; else ok "$first cannot open /.fly/api"; fi
fi

[ "$fail" = 0 ] && echo "K-1293, K-1297: a build cannot reach the service's secrets, another build's, or what the next build runs" || echo "K-1293/K-1297: NOT HELD on this machine"
exit "$fail"
