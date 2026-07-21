# syntax=docker/dockerfile:1
#
# domarinn — multi-stage build producing a tiny, fully static image.
#
#   stage 1 (web)     build the React/Vite UI  -> web/dist
#   stage 2 (builder) compile the static musl binary, embedding web/dist
#   stage 3 (runtime) distroless/static, binary-only, its own healthcheck
#
# The result is a single ~self-contained binary on a scratch-like base: no
# shell, no libc, no package manager, nothing to CVE-scan but the binary.

# ---------------------------------------------------------------------------
# Stage 1: build the web UI. Vite emits static assets into web/dist, which the
# server crate embeds into the binary via rust-embed in stage 2.
# ---------------------------------------------------------------------------
FROM node:24-alpine AS web
WORKDIR /app

# corepack ships pnpm with the node image; enable it and pin via packageManager.
RUN corepack enable

# Copy the web app source. (A lockfile-first copy would cache installs better,
# but the web/ layout is owned by another track; copy it wholesale for
# robustness.)
COPY web/ ./web/

# Frozen install for reproducibility; fall back to a normal install if the
# lockfile is missing or out of date so a fresh checkout still builds.
RUN pnpm -C web install --frozen-lockfile || pnpm -C web install
RUN pnpm -C web build

# ---------------------------------------------------------------------------
# Stage 2: compile the static CLI/server binary against musl.
#
# Toolchain notes:
#   * rusqlite's `bundled` feature compiles SQLite from C, so we need a C
#     toolchain (musl-dev + gcc) even for a "pure Rust" project.
#   * cmake + pkgconfig are here as a hedge: some crypto backends (aws-lc-rs)
#     shell out to cmake. We DO NOT want that — the rustls crypto provider must
#     be pinned to `ring` in the crate config (domarinn-server /
#     domarinn-cli Cargo features) to avoid aws-lc-rs' cmake + arm64 pain.
#     The pin lives with the crate; this comment documents the dependency.
#   * perl is required by ring's build for some targets.
# ---------------------------------------------------------------------------
FROM rust:1-alpine AS builder
WORKDIR /src

RUN apk add --no-cache \
    musl-dev \
    gcc \
    g++ \
    make \
    cmake \
    pkgconfig \
    perl

# The rust:alpine host triple already is x86_64-unknown-linux-musl, so this
# target is present; add it explicitly so the build is deterministic if the
# base image ever changes.
ARG TARGET=x86_64-unknown-linux-musl
RUN rustup target add "${TARGET}"

# Copy the workspace. Cargo.lock is copied so the release build is reproducible.
COPY Cargo.toml Cargo.lock ./
COPY crates/ ./crates/
# Bring in the compiled UI so rust-embed can bake it into the binary.
COPY --from=web /app/web/dist ./web/dist

# Build only the CLI binary (it pulls in core + cache + server). Cache mounts
# keep the registry and target dir warm across builds.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --target "${TARGET}" -p domarinn-cli && \
    cp "target/${TARGET}/release/domarinn" /domarinn

# Empty dir that becomes the runtime image's /data mountpoint. It has to be
# made here: distroless has no shell, so the runtime stage can only COPY it in.
RUN mkdir -p /data

# ---------------------------------------------------------------------------
# Stage 3: runtime. distroless/static has no shell and no libc — a static musl
# binary runs directly, and the binary is its own healthcheck probe.
# ---------------------------------------------------------------------------
FROM gcr.io/distroless/static-debian12:nonroot AS runtime

COPY --from=builder /domarinn /domarinn

# Persistent state (SQLite db, cache) lives under /data. The mountpoint must
# exist in the image owned by the runtime user BEFORE the VOLUME instruction:
# Docker seeds a fresh named volume with the mountpoint's ownership, and a
# root-owned /data leaves the nonroot server unable to create its SQLite
# files. Numeric uid:gid (nonroot is 65532) so no /etc/passwd lookup is needed.
COPY --from=builder --chown=65532:65532 /data /data
ENV DOMARINN_DATA_DIR=/data
VOLUME ["/data"]

EXPOSE 8321
USER nonroot

# The binary probes its own /api/v1/health — distroless has no curl/wget/shell,
# so domarinn is the only thing that can healthcheck domarinn.
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD ["/domarinn", "healthcheck"]

ENTRYPOINT ["/domarinn", "server"]
