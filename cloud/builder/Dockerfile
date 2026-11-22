# The build box: the machine that runs `krate create` when somebody types a
# sentence on krate.tech.
#
# It is a real Linux machine and not a serverless function on purpose. One
# build compiles Rust to WebAssembly for several minutes, spawns processes,
# and writes a couple of hundred megabytes of intermediate object files.
# Nothing about that fits in a request-shaped runtime.
#
# Two decisions worth knowing before changing anything here:
#
# 1. **The engine is the published release, not a build of this repo.** The
#    same signed x86_64 tarball a Linux user downloads. A builder compiled
#    from a branch could make apps that no released Krate can open, and the
#    person would have no way to tell. Pinning to a release means the box can
#    only produce what the shipped runtime can run.
#
# 2. **The Rust toolchain is baked into the image, not installed at boot.**
#    It is ~2 GB and pinned to the same version CI uses. Installing it per
#    machine start would add minutes to the first build after every idle
#    period, which is precisely when somebody is watching.

# Trixie, not Bookworm, and this is load-bearing. The released Linux binary is
# built on ubuntu-latest and links GLIBC 2.38/2.39; Bookworm ships 2.36, so
# the engine installs cleanly and then refuses to start with "version
# GLIBC_2.38 not found". Trixie's 2.41 satisfies it. If a future release
# raises the floor again, this line is the one that moves.
FROM rust:1.94.1-slim-trixie

# What the runtime links against on Linux. Lifted from the CI workflow rather
# than guessed: audio, Wayland, keyboard and udev. `--shoot` renders headless
# with no display server, but the binary still links these at load time.
#
# unzip unpacks the release's notarized gift opener (src/gift.js).
#
# ca-certificates is not optional -- every hub call and crates.io fetch is
# HTTPS, and without it they fail with an error that reads like a network
# outage.
#
# Neither are the fonts, for a reason that only shows up in a picture. A
# slim image has NO fonts at all, so the renderer falls back to a built-in
# bitmap face and every preview of a browser-built app comes out looking
# like a 1980s terminal -- rendered correctly, and wrong in the only way a
# stranger judging the product would notice. DejaVu and Liberation cover
# sans, serif and mono; fontconfig is what lets the renderer find them.
# zstd is how a big-file upload's source archive is read when it is judged
# (IC-945): `tar --zstd` needs the program.
RUN apt-get update -q && apt-get install -y -q --no-install-recommends \
      libasound2-dev \
      libwayland-dev \
      libxkbcommon-dev \
      libudev-dev \
      ca-certificates \
      unzip \
      curl \
      xz-utils \
      zstd \
      fontconfig \
      fonts-dejavu-core \
      fonts-liberation2 \
    && rm -rf /var/lib/apt/lists/*

# Node from nodesource, not Debian's. Bookworm ships Node 18; the service
# is written against 20+ and its package.json says so, so Debian's would
# start and then fail on syntax at the first request.
ARG NODE_VERSION=22.11.0
RUN set -eux; \
    cd /tmp; \
    curl -fsSL -O "https://nodejs.org/dist/v${NODE_VERSION}/node-v${NODE_VERSION}-linux-x64.tar.xz"; \
    tar -xJf "node-v${NODE_VERSION}-linux-x64.tar.xz"; \
    for d in bin include lib share; do \
      cp -r "node-v${NODE_VERSION}-linux-x64/${d}" /usr/local/; \
    done; \
    rm -rf /tmp/node-v*; \
    node --version

# The wasm targets a guest app compiles to. Same list as rust-toolchain.toml.
RUN rustup target add wasm32-wasip1 wasm32-wasip2 wasm32-unknown-unknown

# The engine, and the build tool that ships beside it.
#
# A released asset, verified against the checksum published next to it. This
# is a binary we are about to hand a stranger's words to, and "downloaded
# over HTTPS" is not the same as "the file we meant". The checksum is checked
# under the file's PUBLISHED name, because that is the name SHA256SUMS lists
# -- renaming the download to something convenient makes the check fail with
# a confusing "no such file".
#
# cargo-component comes out of the same tarball rather than `cargo install`.
# Two reasons, both real: installing it from source costs several minutes of
# image build, and the shipped one is by construction the version this exact
# engine was released against. A mismatch there produces components that
# build fine and fail to run.
ARG KRATE_VERSION=0.6.1
RUN set -eux; \
    base="https://github.com/incyashraj/krate/releases/download/v${KRATE_VERSION}"; \
    file="krate-${KRATE_VERSION}-x86_64-unknown-linux-gnu.tar.gz"; \
    cd /tmp; \
    curl -fsSL -O "${base}/${file}"; \
    curl -fsSL -O "${base}/SHA256SUMS"; \
    grep " ${file}\$" SHA256SUMS | sha256sum -c -; \
    tar -xzf "${file}"; \
    dir="krate-${KRATE_VERSION}-x86_64-unknown-linux-gnu"; \
    install -m 0755 "${dir}/krate" /usr/local/bin/krate; \
    install -m 0755 "${dir}/cargo-component" /usr/local/bin/cargo-component; \
    rm -rf "/tmp/${file}" /tmp/SHA256SUMS "/tmp/${dir}"; \
    krate --version; \
    cargo-component --version

# The C compiler for WebAssembly (IC-941, K-1072). An app whose crates
# compile C -- ring for TLS and QUIC, rusqlite bundled, zstd -- needs a
# clang that targets WASI and the WASI libc beside it; the image's own cc
# has neither and fails with "'stdio.h' file not found". The same wasi-sdk
# release `krate setup wasi-sdk` pins, verified before it is unpacked.
#
# The digest is the one crates/cli/src/wasi_sdk.rs pins for x86_64 Linux,
# printed by scripts/wasi-sdk-checksums.py and matching GitHub's own for the
# asset; a unit test there fails if the two drift. Moving to another release
# means a new version, tag and digest together.
ARG WASI_SDK_VERSION=34.0
ARG WASI_SDK_TAG=wasi-sdk-34
ARG WASI_SDK_SHA256=b761e3a0721dbae9c09a0059e5fdb2bf917d1b4a8a7b430fb3b5aafb0984b2c4
RUN set -eux; \
    file="wasi-sdk-${WASI_SDK_VERSION}-x86_64-linux.tar.gz"; \
    cd /tmp; \
    curl -fsSL -O "https://github.com/WebAssembly/wasi-sdk/releases/download/${WASI_SDK_TAG}/${file}"; \
    echo "${WASI_SDK_SHA256}  ${file}" | sha256sum -c -; \
    tar -xzf "${file}"; \
    mv "wasi-sdk-${WASI_SDK_VERSION}-x86_64-linux" /opt/wasi-sdk; \
    rm -f "/tmp/${file}"; \
    /opt/wasi-sdk/bin/clang --version

# Krate engines from IC-941 on find it through WASI_SDK_PATH. The released
# engine this image pins may predate that, so cc-rs is also told directly;
# a build passes these through to cargo either way.
ENV WASI_SDK_PATH=/opt/wasi-sdk \
    CC_wasm32_wasip1=/opt/wasi-sdk/bin/clang \
    CXX_wasm32_wasip1=/opt/wasi-sdk/bin/clang++ \
    AR_wasm32_wasip1=/opt/wasi-sdk/bin/llvm-ar \
    CFLAGS_wasm32_wasip1="--target=wasm32-wasip1 --sysroot=/opt/wasi-sdk/share/wasi-sysroot" \
    CXXFLAGS_wasm32_wasip1="--target=wasm32-wasip1 --sysroot=/opt/wasi-sdk/share/wasi-sysroot"

WORKDIR /srv
COPY package.json ./
COPY src ./src
COPY entrypoint.sh /usr/local/bin/entrypoint.sh
# The post-deploy check for K-1293: `fly ssh console -C krate-isolation-check`.
COPY isolation-check.sh /usr/local/bin/krate-isolation-check

# The service and a pool of build users: the line between them is K-1293,
# the lines between the build users are K-1297.
#
# The SERVICE runs as root. It holds the hub secret and the Anthropic key in
# its environment, and it runs none of a stranger's code itself.
#
# Every BUILD runs as a build user: the engine, the AI agent loop inside it,
# cargo, rustc, every dependency's build script and proc-macro, the app when
# it is photographed, the judge reading an upload. All of that is code shaped
# by a stranger's words or upload. When it ran as the service's own user it
# could read /proc/<service>/environ -- `include_str!("/proc/1/environ")` was
# enough -- and carry the secrets out in a bundle or a compiler error. A
# process that root handed to another uid has no ptrace rights over root, so
# root's environ, and /work/state (root's, 0700), are out of its reach.
#
# One build user for every build was not enough (K-1297): two builds at once
# could read each other's environment (another person's own API key) and
# folders. So there are BUILD_SLOTS of them, kratebuild1..N, and a running
# job leases one for its life (src/sandbox.js): no two running jobs share a
# uid, and the uid's home (/work/build/<n>, 0700) is emptied, after every
# process of that uid is killed, before the next job gets it. N is also how
# many jobs run at once; a job beyond it is told the box is busy.
#
# Root, not an unprivileged service user with CAP_SETUID: capabilities
# granted that way survive the child's setuid and its exec, so the child
# could switch straight back. A switch FROM root clears every capability.
#
# Each build user: its own group and nothing else, no login shell, no sudo
# in the image. Every set-uid and set-gid bit in the image is cleared so no
# program can lift a build back to root.
#
# What every build shares and none may change: the toolchain (RUSTUP_HOME),
# the image's cargo home (cargo's bin, and nothing a build writes: each
# build gets a CARGO_HOME of its own), the engine and cargo-component in
# /usr/local/bin, and wasi-sdk. The rust base image ships RUSTUP_HOME and
# CARGO_HOME world-writable (chmod -R a+w), so a build could have replaced
# cargo, rustc or a crate for every build after it. Root's here, and not
# group- or other-writable; the service refuses to build where that does
# not hold (sandbox.js, sharedWritable).
#
# The last line proves, at image build, that every build user can run every
# tool a build uses -- with a CARGO_HOME of its own, as a build gets -- that
# none can write anything shared, and that none can read PID 1's
# environment. If a base image change breaks any of it, the image does not
# build -- rather than the first person's build failing on the live box.
ARG BUILD_SLOTS=6
RUN set -eu; \
    i=1; while [ "$i" -le "${BUILD_SLOTS}" ]; do \
      useradd --uid "$((10000 + i))" --user-group --no-create-home --home-dir "/work/build/$i" \
        --shell /usr/sbin/nologin "kratebuild$i"; \
      i=$((i + 1)); \
    done; \
    chmod 0755 /usr/local/bin/entrypoint.sh /usr/local/bin/krate-isolation-check; \
    find / -xdev -type f -perm /6000 -exec chmod ug-s {} +; \
    chown -R root:root "$RUSTUP_HOME" "$CARGO_HOME" /usr/local/bin /opt/wasi-sdk; \
    chmod -R go-w "$RUSTUP_HOME" "$CARGO_HOME" /usr/local/bin /opt/wasi-sdk; \
    mkdir -p /work; \
    i=1; while [ "$i" -le "${BUILD_SLOTS}" ]; do \
      HOME=/tmp setpriv --reuid="kratebuild$i" --regid="kratebuild$i" --clear-groups sh -euc '\
        home="$(mktemp -d)"; export HOME="$home" CARGO_HOME="$home/.cargo"; \
        mkdir "$CARGO_HOME"; ln -s /usr/local/cargo/bin "$CARGO_HOME/bin"; \
        krate --version >/dev/null; \
        cargo-component --version >/dev/null; \
        cargo --version >/dev/null; \
        rustup which cargo >/dev/null; \
        rustup target list --installed | grep -qx wasm32-wasip1; \
        /opt/wasi-sdk/bin/clang --version >/dev/null; \
        for p in /usr/local/rustup /usr/local/cargo /usr/local/bin /opt/wasi-sdk; do \
          w="$(find "$p" -writable -print -quit 2>/dev/null)"; \
          if [ -n "$w" ]; then echo "$(id -un) can write $w" >&2; exit 1; fi; \
        done; \
        if cat /proc/1/environ >/dev/null 2>&1; then echo "$(id -un) can read PID 1s environment" >&2; exit 1; fi; \
        rm -rf "$home"; \
        echo "$(id -un): toolchain ok with its own cargo home, nothing shared writable, PID 1 environ unreadable"'; \
      i=$((i + 1)); \
    done

# Where builds happen. A Fly volume mounts at /work: the service's state
# (jobs, finished apps, ledgers) survives a machine stopping; the build
# users' homes under /work/build do not need to, and are emptied at boot.
#
# CARGO_HOME deliberately stays where the base image put it, for the
# service and for the image check above. Moving it to the volume looks tidy
# and breaks the build: rustup and the pinned toolchain live under
# /usr/local, `krate` shells out to `rustup` to find the toolchain with the
# wasm targets, and a CARGO_HOME on an empty volume orphans all of it --
# producing "failed to find the wasm32-wasip1 target and rustup is not
# available" on a machine that plainly has both.
#
# A BUILD gets a CARGO_HOME of its own (sandbox.js): <its home>/.cargo, with
# bin/ a link to the image's (rustup finds the toolchain through RUSTUP_HOME
# and PATH, which stay), so the crates it fetches land in its own home and
# go with it. A registry shared between builds is a registry one build can
# plant a crate in for the next. A read-only, pre-filled one would hold only
# the crates known when the image was built, and an AI build picks crates
# while it works -- so each build fetches its own (crates.io's sparse index,
# a few seconds), and nothing it fetches outlives it.
#
# The engine keeps its SDK and its compile cache under $HOME/.cache/krate,
# and a build's HOME is its lease's home, emptied after every job: each
# build lays the SDK down from the engine binary (a few MB) and compiles the
# SDK's crates itself. That costs the warm cache a second build used to
# get; it is what leaves nothing one build writes where another build reads.
#
# Do NOT set CARGO_TARGET_DIR here to "help". The engine treats an explicit
# one as the person overriding it and switches its own fingerprinted cache
# off -- so the well-meant setting would replace a correct cache with a
# blunter one that cannot tell SDK versions apart.
#
# KRATE_MAKER_SIGNING=off: this box makes apps for everybody, and Krate signs
# what a person makes with their own publisher key (IC-983). One key here
# would sign every account's apps as one publisher, so it signs nothing
# itself: the hub signs each finished build as that person's account
# (/sign/web-build, IC-983 option B), and the key never comes here.
#
# KRATE_BUILD_USER and KRATE_BUILD_SLOTS name the pool (kratebuild1..N,
# src/sandbox.js); with FLY_APP_NAME set, a box without all of them, or with
# a shared path a build user could write, refuses every build instead of
# running it as root or on a poisonable toolchain.
# KRATE_JUDGE_DIR is where the hub's big uploads land for judging: root's,
# 0711, so the judge (as a build user) reads a file by its exact path but can
# neither list the folder nor swap a file after its hash was checked.
ENV HOME=/work \
    TMPDIR=/work/tmp \
    PORT=8080 \
    KRATE_MAKER_SIGNING=off \
    KRATE_BUILD_USER=kratebuild \
    KRATE_BUILD_SLOTS=${BUILD_SLOTS} \
    KRATE_JUDGE_DIR=/work/judge

EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
