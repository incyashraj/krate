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
RUN apt-get update -q && apt-get install -y -q --no-install-recommends \
      libasound2-dev \
      libwayland-dev \
      libxkbcommon-dev \
      libudev-dev \
      ca-certificates \
      curl \
      xz-utils \
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
ARG KRATE_VERSION=0.3.0
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

WORKDIR /srv
COPY package.json ./
COPY src ./src
COPY entrypoint.sh /usr/local/bin/entrypoint.sh

# The service does not run as root: it spawns a compiler and an AI agent on
# a stranger's words, and the blast radius of a bad day should not include
# the machine. The container still STARTS as root, because a freshly created
# Fly volume arrives root-owned and something has to chown it -- entrypoint.sh
# does that and then drops to this user for good.
RUN useradd --create-home --shell /bin/sh builder \
    && chmod 0755 /usr/local/bin/entrypoint.sh \
    && mkdir -p /work \
    && chown -R builder:builder /srv /work

# Where builds happen. A Fly volume mounts at /work, so scratch build dirs
# and the downloaded-crate cache survive a machine stopping -- the difference
# between a warm second build and re-fetching crates.io on every wake.
#
# CARGO_HOME deliberately stays where the base image put it. Moving it to the
# volume looks tidy and breaks the build: rustup and the pinned toolchain
# live under /usr/local, `krate` shells out to `rustup` to find the toolchain
# with the wasm targets, and a CARGO_HOME on an empty volume orphans all of
# it -- producing "failed to find the wasm32-wasip1 target and rustup is not
# available" on a machine that plainly has both.
#
# HOME does the work instead, and that is enough: the engine already keeps a
# shared dependency cache at $HOME/.cache/krate/build, keyed by SDK
# fingerprint so a WIT change can never serve stale artifacts. Pointing HOME
# at the volume puts that cache on the volume.
#
# Do NOT set CARGO_TARGET_DIR here to "help". The engine treats an explicit
# one as the person overriding it and switches its own fingerprinted cache
# off -- so the well-meant setting would replace a correct cache with a
# blunter one that cannot tell SDK versions apart.
ENV HOME=/work \
    TMPDIR=/work/tmp \
    PORT=8080

EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
