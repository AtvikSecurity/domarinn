# syntax=docker/dockerfile:1
#
# domarinn — multi-stage build producing a small, mostly-static image.
#
#   stage 1 (web)      build the React/Vite UI  -> web/dist
#   stage 2 (cdeps)    build static openssl/libxml2/xmlsec1 (native glibc)
#   stage 3 (builder)  compile the binary, embedding web/dist
#   stage 4 (runtime)  distroless/cc, binary-only, its own healthcheck
#
# The binary statically links every C dependency (openssl, libxml2, xmlsec1,
# bundled SQLite) and dynamically links ONLY glibc, which distroless/cc
# provides. The CVE surface is the binary plus distroless's glibc/libgcc,
# patched by rebasing on base-image updates (Renovate watches the tags).
#
# SAML support (the `saml` cargo feature) needs samael's xmlsec backend, which
# links libxmlsec1/libxml2/openssl at build time. Those are built from pinned
# tarballs rather than apt because Debian only packages xmlsec 1.2.x (bookworm
# 1.2.37, trixie 1.2.41) and samael needs 1.3.x (issue #82: 1.2.3x breaks its
# ID registration), and because our libxml2 is configured minimal (no
# zlib/lzma/http), dropping whole attack-surface classes Debian's build keeps.
#
# History: this used to be a fully-static musl cross-build on distroless/
# static. The musl-gcc cross machinery (kernel-header symlinks, stub archive
# workarounds, per-arch casing) cost more than the ~22MB of glibc base image
# it saved; a native glibc build needs none of it.

# ---------------------------------------------------------------------------
# Stage 1: build the web UI. Vite emits static assets into web/dist, which the
# server crate embeds into the binary via rust-embed in the builder stage.
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
# Stage 2: build static OpenSSL + libxml2 + xmlsec1, native glibc.
#
# Pinned tarball versions (bump + rebuild on upstream security releases;
# Renovate watches the ARGs). xmlsec is pinned to 1.3.x — samael issue #82
# reports 1.2.3x breaks its ID registration. Static archives only; the
# builder stage links them into the binary. Native builds, so ./config //
# ./configure autodetect the platform — the same Dockerfile serves amd64 and
# arm64 without cross-compile casing.
#
# The downloads retry on ANY error, 5xx included (--retry-all-errors; bare
# --retry covers only timeouts/connection resets). These layers are normally
# served from the buildx cache, so a fetch only actually runs on a cache miss
# — exactly when a transient upstream 503 would otherwise fail the whole
# build, as one did when GitHub's release CDN blipped.
# ---------------------------------------------------------------------------
FROM debian:bookworm-slim AS cdeps
ARG OPENSSL_VER=3.5.1
ARG LIBXML2_VER=2.13.5
ARG XMLSEC_VER=1.3.7

RUN apt-get update && apt-get install -y --no-install-recommends \
    gcc libc6-dev make perl pkg-config curl ca-certificates xz-utils \
    && rm -rf /var/lib/apt/lists/*

ENV CDEPS_PREFIX=/usr/local/cdeps
ENV PKG_CONFIG_PATH=${CDEPS_PREFIX}/lib/pkgconfig
ENV PATH=${CDEPS_PREFIX}/bin:${PATH}
WORKDIR /build

# 1. OpenSSL — no shared libs, no engines/dso; the same install feeds both
#    xmlsec1's configure and the Rust build's OPENSSL_DIR.
RUN curl -fsSL --retry 5 --retry-all-errors --retry-delay 2 "https://github.com/openssl/openssl/releases/download/openssl-${OPENSSL_VER}/openssl-${OPENSSL_VER}.tar.gz" -o openssl.tar.gz && \
    tar xf openssl.tar.gz && cd "openssl-${OPENSSL_VER}" && \
    ./config no-shared no-dso no-engine no-tests \
      --prefix="$CDEPS_PREFIX" --openssldir="$CDEPS_PREFIX/ssl" --libdir=lib && \
    make -j"$(nproc)" && make install_sw && cd .. && rm -rf "openssl-${OPENSSL_VER}" openssl.tar.gz

# 2. libxml2 — minimal: no python/zlib/lzma/http, kills large attack-surface
#    classes we never touch. Static only.
RUN curl -fsSL --retry 5 --retry-all-errors --retry-delay 2 "https://download.gnome.org/sources/libxml2/${LIBXML2_VER%.*}/libxml2-${LIBXML2_VER}.tar.xz" -o libxml2.tar.xz && \
    tar xf libxml2.tar.xz && cd "libxml2-${LIBXML2_VER}" && \
    ./configure --prefix="$CDEPS_PREFIX" \
      --enable-static --disable-shared \
      --without-python --without-zlib --without-lzma --without-http && \
    make -j"$(nproc)" && make install && cd .. && rm -rf "libxml2-${LIBXML2_VER}" libxml2.tar.xz

# 3. xmlsec1 — static, crypto statically bound to OpenSSL (no runtime dlopen),
#    no XSLT (removes the entire XSLT-transform class), OpenSSL only.
RUN curl -fsSL --retry 5 --retry-all-errors --retry-delay 2 "https://github.com/lsh123/xmlsec/releases/download/${XMLSEC_VER}/xmlsec1-${XMLSEC_VER}.tar.gz" -o xmlsec1.tar.gz && \
    tar xf xmlsec1.tar.gz && cd "xmlsec1-${XMLSEC_VER}" && \
    ./configure --prefix="$CDEPS_PREFIX" \
      --enable-static --disable-shared --enable-crypto-dl=no \
      --without-libxslt --without-gnutls --without-nss --with-openssl="$CDEPS_PREFIX" \
      --disable-apps --disable-docs && \
    make -j"$(nproc)" && make install && cd .. && rm -rf "xmlsec1-${XMLSEC_VER}" xmlsec1.tar.gz

# ---------------------------------------------------------------------------
# Stage 3: compile the CLI/server binary, native glibc.
#
# Toolchain notes:
#   * clang/libclang for bindgen (samael's build script).
#   * The rustls crypto provider must stay pinned to `ring` (never aws-lc-rs)
#     to avoid cmake + arm64 pain; verify with `cargo tree`. The static
#     OpenSSL from stage 2 enters ONLY via samael and coexists with ring
#     (ring 0.17 prefixes its symbols).
#   * rusqlite's `bundled` SQLite compiles natively with the host gcc.
# ---------------------------------------------------------------------------
FROM rust:1-bookworm AS builder
WORKDIR /src

RUN apt-get update && apt-get install -y --no-install-recommends \
    clang libclang-dev make perl pkg-config \
    && rm -rf /var/lib/apt/lists/*

# The static C libs built in stage 2.
COPY --from=cdeps /usr/local/cdeps /usr/local/cdeps

ENV CDEPS_PREFIX=/usr/local/cdeps
ENV PKG_CONFIG_PATH=${CDEPS_PREFIX}/lib/pkgconfig
ENV PKG_CONFIG_ALL_STATIC=1
ENV OPENSSL_DIR=${CDEPS_PREFIX}
ENV OPENSSL_STATIC=1
# xmlsec1-config on PATH so samael's build script finds it and emits static
# link directives (it branches on the absence of XMLSEC_CRYPTO_DYNAMIC_LOADING).
ENV PATH=${CDEPS_PREFIX}/bin:${PATH}

# Copy the workspace. Cargo.lock is copied so the release build is reproducible.
COPY Cargo.toml Cargo.lock ./
COPY crates/ ./crates/
# Bring in the compiled UI so rust-embed can bake it into the binary.
COPY --from=web /app/web/dist ./web/dist

# Build only the CLI binary (it pulls in core + cache + server) with SAML on.
# The gate asserts every C dependency linked statically: the only dynamic
# libraries allowed are glibc's own (libc/libm/libgcc_s + the loader), which
# distroless/cc provides. A stray libxml2/libssl/libxmlsec `=>` line fails
# the build.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --features saml -p domarinn-cli && \
    cp target/release/domarinn /domarinn && \
    ldd /domarinn && \
    ! ldd /domarinn | grep '=>' | grep -vE '/lib(c|m|gcc_s)\.so|ld-linux|linux-vdso' && \
    echo "glibc-only OK"

# Empty dir that becomes the runtime image's /data mountpoint. It has to be
# made here: distroless has no shell, so the runtime stage can only COPY it in.
RUN mkdir -p /data

# ---------------------------------------------------------------------------
# Stage 4: runtime. distroless/cc has glibc (and nothing else — no shell, no
# package manager); its debian12 tag matches the bookworm builder's glibc.
# ---------------------------------------------------------------------------
FROM gcr.io/distroless/cc-debian12:nonroot AS runtime

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
