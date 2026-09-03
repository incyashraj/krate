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
set -eu

for dir in /work/tmp /work/.cache /work/.cargo /work/.krate; do
  mkdir -p "$dir"
done

# Drop to the unprivileged user for the actual service.
#
# setpriv rather than su, and this matters more than it looks. `su` stays
# alive as the parent, so it -- not node -- ends up as PID 1, and signals
# sent to the container go to `su` instead of the service. A build would be
# killed abruptly on every deploy rather than shut down. setpriv exec's
# straight into node, so node IS PID 1 and gets the signal itself.
if [ "$(id -u)" = "0" ]; then
  chown -R builder:builder /work
  exec setpriv --reuid=builder --regid=builder --init-groups \
    node /srv/src/server.js
fi

exec node /srv/src/server.js
