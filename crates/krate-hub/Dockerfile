# Krate Hub -- multi-stage build.
#
# IMPORTANT: the build context is the REPO ROOT, not this directory, because
# krate-hub is a workspace member and its dependencies (and the workspace
# Cargo.toml / Cargo.lock) live at the root. Build it from the repo root with:
#
#   docker build -f crates/krate-hub/Dockerfile -t krate-hub .
#
# See crates/krate-hub/DEPLOY.md for the full run command.

# ---- build stage -------------------------------------------------------------
# Pin to the toolchain the workspace uses (see rust-toolchain.toml: 1.91.1).
FROM rust:1.91-slim AS build
WORKDIR /src

# Copy the whole workspace. A .dockerignore at the repo root keeps this to the
# source that actually matters (target/, .git, node_modules, etc. excluded).
COPY . .

# Build only the hub, in release. --locked keeps the build reproducible against
# the committed Cargo.lock.
RUN cargo build --release -p krate-hub --locked

# ---- runtime stage -----------------------------------------------------------
# debian:bookworm-slim is tiny, glibc-based (matches the rust:slim build), and
# gives us a shell + a way to create a non-root user. A distroless base would be
# smaller still but makes creating the data dir + user awkward; slim is the
# pragmatic v1 choice.
FROM debian:bookworm-slim AS runtime

# Run as a non-root user. The data dir is created and owned by it so the
# volume-mounted store is writable without running as root.
RUN groupadd --system krate \
 && useradd --system --gid krate --home-dir /data --no-create-home krate \
 && mkdir -p /data \
 && chown krate:krate /data

COPY --from=build /src/target/release/krate-hub /usr/local/bin/krate-hub

# Bind all interfaces inside the container so the port is reachable from the
# host / the platform's proxy. KRATE_HUB_PUBLIC_URL is set by the deployment
# (fly.toml / docker run) to the real public origin; the default here is only a
# sane fallback for a bare `docker run` without -e.
ENV KRATE_HUB_ADDR=0.0.0.0:8787 \
    KRATE_HUB_DIR=/data \
    KRATE_HUB_PUBLIC_URL=http://localhost:8787

# Persist published apps across container restarts / redeploys.
VOLUME ["/data"]

EXPOSE 8787

USER krate
WORKDIR /data

ENTRYPOINT ["/usr/local/bin/krate-hub"]
