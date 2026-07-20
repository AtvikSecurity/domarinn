# syntax=docker/dockerfile:1
#
# measurellm — multi-stage build producing a tiny, fully static image.
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
FROM node:22-alpine AS web
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
#     be pinned to `ring` in the crate config (measurellm-server /
#     measurellm-cli Cargo features) to avoid aws-lc-rs' cmake + arm64 pain.
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
    cargo build --release --target "${TARGET}" -p measurellm-cli && \
    cp "target/${TARGET}/release/measurellm" /measurellm

# ---------------------------------------------------------------------------
# Stage 3: runtime. distroless/static has no shell and no libc — a static musl
# binary runs directly, and the binary is its own healthcheck probe.
# ---------------------------------------------------------------------------
FROM gcr.io/distroless/static-debian12:nonroot AS runtime

COPY --from=builder /measurellm /measurellm

# Persistent state (SQLite db, cache) lives under /data.
ENV MEASURELLM_DATA_DIR=/data
VOLUME ["/data"]

EXPOSE 8321
USER nonroot

# The binary probes its own /api/v1/health — distroless has no curl/wget/shell,
# so measurellm is the only thing that can healthcheck measurellm.
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD ["/measurellm", "healthcheck"]

ENTRYPOINT ["/measurellm", "server"]
