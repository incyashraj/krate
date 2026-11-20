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
# Two owners on the volume (K-1293, K-1297, see the Dockerfile):
#
#   /work            root 0755   the way in, nothing more
#   /work/state      root 0700   jobs, finished apps, ledgers: the service's
#   /work/tmp        root 0700   the service's own scratch
#   /work/judge      root 0711   uploads being judged: read by exact path only
#   /work/build      root 0755   the build users' homes, one per slot:
#   /work/build/<n>  kratebuild<n> 0700, made fresh for each job and emptied
#                    after it (src/sandbox.js)
#
# A build runs as the build user its job leased and so can reach its own
# home and nothing else: not the service's files, not another job's.
set -eu

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

# The build users' homes start empty on every boot: nothing a build wrote
# in an earlier life of this machine (a compile cache, a fetched crate, a
# job's files) is there for the next one (K-1297). That includes the old
# layout's shared caches, /work/build/.cache and /work/.cache, which every
# build could write.
rm -rf /work/build /work/.cache
mkdir -p /work/build
chown root:root /work/build
chmod 0755 /work/build

# Node runs as root and is this container's main process, so it gets the
# stop signal itself and ends the builds in flight politely (server.js).
exec node /srv/src/server.js
