# syntax=docker/dockerfile:1

# ---------------------------------------------------------------------------
# Builder: compile the headless server only (the GUI client is excluded).
# ---------------------------------------------------------------------------
FROM rust:1.92-bookworm AS builder

# sqlx links the system SQLite, so the dev headers are required. pkg-config is
# used to discover the library.
RUN apt-get update \
    && apt-get install -y --no-install-recommends pkg-config libsqlite3-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /src

# Layer 1: copy only the manifests and lockfile, then build a stub with the
# real dependency graph so Docker can cache compiled dependencies.
COPY Cargo.toml Cargo.lock ./
COPY crates/flex-protocol/Cargo.toml crates/flex-protocol/Cargo.toml
COPY crates/flex-server/Cargo.toml crates/flex-server/Cargo.toml
COPY crates/flex-client/Cargo.toml crates/flex-client/Cargo.toml

RUN mkdir -p crates/flex-protocol/src crates/flex-server/src crates/flex-client/src \
    && printf '' > crates/flex-protocol/src/lib.rs \
    && printf 'fn main() {}\n' > crates/flex-server/src/main.rs \
    && printf 'fn main() {}\n' > crates/flex-client/src/main.rs \
    && cargo build --release -p flex-server \
    && rm -rf crates/*/src

# Layer 2: copy the real sources and build the final binary.
COPY crates ./crates
RUN cargo build --release -p flex-server

# ---------------------------------------------------------------------------
# Runtime: slim image with only the server and its runtime dependencies.
# ---------------------------------------------------------------------------
FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ffmpeg libsqlite3-0 ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /src/target/release/flex-server /usr/local/bin/flex-server

RUN mkdir -p /etc/flex /data

EXPOSE 8096
VOLUME ["/data"]

# First-run admin bootstrap (unset after the account exists):
#   -e FLEX_ADMIN_USER=admin -e FLEX_ADMIN_PASSWORD=...
# Alternatively create accounts with the CLI, e.g.:
#   docker run --rm flex-server user add admin --admin --password '...'
#
# Hardware transcoding (NVENC) is OFF by default: the image runs CPU-only with
# the libx264 software fallback. To enable it, pass an NVIDIA GPU through
# (requires nvidia-container-toolkit), e.g. `docker run --gpus all ...`.
# NVENC is only used when the GPU is visible to the process.

ENTRYPOINT ["/usr/local/bin/flex-server"]
CMD ["--config", "/etc/flex/flex.toml"]
