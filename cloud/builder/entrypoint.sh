#!/bin/sh
#
# Prepare the volume, then hand off to the service.
#
# A Fly volume arrives EMPTY and owned by root on the first boot, mounted
# over whatever the image had at /work. So every directory the build needs
# has to be made here, at run time, rather than in the Dockerfile where it
# would simply be hidden by the mount. This is the classic volume trap: the
# image looks right, and the running machine has an empty root-owned
# directory the service cannot write to.
#
# Two owners on the volume (K-1293, see the Dockerfile):
#
#   /work            root 0755   the way in, nothing more
#   /work/state      root 0700   jobs, finished apps, ledgers: the service's
#   /work/tmp        root 0700   the service's own scratch
#   /work/judge      root 0711   uploads being judged: read by exact path only
#   /work/build      kratebuild 0700   the build user's home: caches, scratch
#
# A build runs as kratebuild and so can reach its own home and nothing of
# the service's: not another person's finished app, not a ledger.
set -eu

BUILD_USER="${KRATE_BUILD_USER:-kratebuild}"

if [ "$(id -u)" != "0" ]; then
  # Started as somebody else (`docker run --user`): nothing here can hand a
  # build to another user, and the service refuses builds in production
  # rather than run them as itself.
  exec node /srv/src/server.js
fi

mkdir -p /work/state /work/tmp /work/judge
chown root:root /work /work/state /work/tmp /work/judge
chmod 0755 /work
chmod 0700 /work/state /work/tmp
chmod 0711 /work/judge
# Older images ran the service as `builder` (uid 1000) and left its files
# here. They are the service's now.
chown -R root:root /work/state /work/tmp

home="$(getent passwd "$BUILD_USER" | cut -d: -f6 || true)"
if [ -n "$home" ]; then
  mkdir -p "$home"
  # The warm cache the old layout kept at /work/.cache (the SDK, the
  # compiled-dependency cache) moves once into the build user's home, so
  # the first build after this change is not a cold one.
  if [ -d /work/.cache ] && [ ! -e "$home/.cache" ]; then
    mv /work/.cache "$home/.cache"
  fi
  mkdir -p "$home/tmp" "$home/.cache" "$home/.krate"
  chown -R "$BUILD_USER:$BUILD_USER" "$home"
  chmod 0700 "$home"
fi

# Node runs as root and is this container's main process, so it gets the
# stop signal itself and ends the builds in flight politely (server.js).
exec node /srv/src/server.js
