#!/bin/sh
#
# K-1293 on the live builder: can a build read the service's secrets?
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

user="${KRATE_BUILD_USER:-kratebuild}"
as_build() { setpriv --reuid="$user" --regid="$user" --clear-groups "$@"; }

[ "$(id -u)" = "0" ] || { echo "run this as root on the builder"; exit 2; }
getent passwd "$user" >/dev/null && ok "the build user $user exists" || bad "there is no $user user"

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

# The build user, reaching for the service's files and environment.
for target in "/proc/$pid/environ" /proc/1/environ; do
  if as_build cat "$target" >/dev/null 2>&1; then bad "$user can read $target"; else ok "$user cannot read $target"; fi
done
if as_build ls /work/state >/dev/null 2>&1; then bad "$user can list /work/state (other people's apps)"; else ok "$user cannot list /work/state"; fi
if as_build ls /work/judge >/dev/null 2>&1; then bad "$user can list /work/judge"; else ok "$user cannot list /work/judge"; fi
if as_build sh -c 'echo x > /work/judge/probe' 2>/dev/null; then bad "$user can write /work/judge"; rm -f /work/judge/probe; else ok "$user cannot write /work/judge"; fi

# A child started exactly the way the server starts one (src/sandbox.js),
# trying everything: both environs, its own environment, its uid.
out="$(cd /srv && node --input-type=module -e "
  import { createSandbox } from '/srv/src/sandbox.js';
  const box = createSandbox({ log: { log() {}, error() {} } });
  if (!box.isolated) { console.log('NOT-ISOLATED'); process.exit(0); }
  const p = box.spawn('sh', ['-c', 'cat /proc/$pid/environ /proc/1/environ 2>/dev/null; env; echo UID=\$(id -u); ls /work/state 2>/dev/null && echo LISTED-STATE'], { stdio: ['ignore', 'pipe', 'pipe'] });
  let s = ''; p.stdout.on('data', (b) => { s += b; }); p.stderr.on('data', (b) => { s += b; });
  p.on('close', () => process.stdout.write(s));
" 2>&1)"
case "$out" in *NOT-ISOLATED*) bad "the sandbox says builds are not isolated on this machine";; esac
build_uid="$(printf '%s\n' "$out" | sed -n 's/^UID=//p')"
want_uid="$(id -u "$user")"
[ "$build_uid" = "$want_uid" ] && ok "a sandboxed child runs as uid $build_uid" || bad "a sandboxed child ran as uid '${build_uid}', not $want_uid"
if [ -n "$secret" ] && printf '%s' "$out" | grep -qF "$secret"; then bad "a sandboxed child SAW THE SECRET"; else ok "a sandboxed child did not see the secret"; fi
printf '%s\n' "$out" | grep -q '^KRATE_BUILDER_SECRET=' && bad "KRATE_BUILDER_SECRET is in a child's environment" || ok "KRATE_BUILDER_SECRET is not in a child's environment"
printf '%s\n' "$out" | grep -q '^ANTHROPIC_API_KEY=' && bad "a keyless child got ANTHROPIC_API_KEY" || ok "a keyless child has no model key"
printf '%s\n' "$out" | grep -q 'LISTED-STATE' && bad "a sandboxed child listed /work/state" || ok "a sandboxed child cannot list /work/state"

# What the service says about itself.
if curl -fsS "http://127.0.0.1:${PORT:-8080}/health" | grep -q '"isolation":"build-user"'; then ok "/health says isolation: build-user"; else bad "/health does not say isolation: build-user"; fi

# Fly's own socket inside the machine: reported, not judged. If a build can
# reach it, what it can ask for is the next thing to check.
if [ -e /.fly/api ]; then
  if as_build test -r /.fly/api -a -w /.fly/api; then note "$user can open /.fly/api (Fly's machine API socket): check what it answers"; else ok "$user cannot open /.fly/api"; fi
fi

[ "$fail" = 0 ] && echo "K-1293: a build cannot reach the service's secrets on this machine" || echo "K-1293: NOT HELD on this machine"
exit "$fail"
