# syntax=docker/dockerfile:1
#
# domarinn — multi-stage build producing a tiny, fully static image.
#
#   stage 1 (web)      build the React/Vite UI  -> web/dist
#   stage 2 (cdeps)    build static openssl/libxml2/xmlsec1 for musl
#   stage 3 (builder)  compile the static musl binary, embedding web/dist
#   stage 4 (runtime)  distroless/static, binary-only, its own healthcheck
#
# The result is a single ~self-contained binary on a scratch-like base: no
# shell, no libc, no package manager, nothing to CVE-scan but the binary.
#
# SAML support (the `saml` cargo feature) needs samael's xmlsec backend, which
# links libxmlsec1/libxml2/openssl at build time. Those have no static musl
# packages on Alpine, and bindgen cannot dlopen libclang from a static-musl
# host — so the builder runs on glibc (Debian) and cross-compiles to musl,
# with the three C libraries built statically from pinned tarballs.

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
# Stage 2: build static OpenSSL + libxml2 + xmlsec1 against musl.
#
# Pinned tarball versions (bump + rebuild on upstream security releases;
# Renovate watches the ARGs). xmlsec is pinned to 1.3.x — samael issue #82
# reports 1.2.3x breaks its ID registration. All three are built with musl-gcc
# and installed into $MUSL_PREFIX; the builder stage links them statically.
# ---------------------------------------------------------------------------
FROM debian:bookworm-slim AS cdeps
ARG TARGETARCH
ARG OPENSSL_VER=3.5.1
ARG LIBXML2_VER=2.13.5
ARG XMLSEC_VER=1.3.7

RUN apt-get update && apt-get install -y --no-install-recommends \
    musl-tools musl-dev linux-libc-dev make perl pkg-config curl ca-certificates xz-utils \
    && rm -rf /var/lib/apt/lists/*

# Debian's `musl-gcc` wrapper does not put the Linux kernel headers
# (linux/*, asm/*, asm-generic/*) on musl's include path, so anything using
# them (OpenSSL's mem_sec.c, …) fails to compile. Symlink them from
# linux-libc-dev into musl's include dir. Arch-aware for amd64 + arm64.
RUN set -eux; \
    case "$TARGETARCH" in \
      amd64) MUSL_TRIPLE=x86_64-linux-musl; GNU_TRIPLE=x86_64-linux-gnu ;; \
      arm64) MUSL_TRIPLE=aarch64-linux-musl; GNU_TRIPLE=aarch64-linux-gnu ;; \
      *) echo "unsupported TARGETARCH=$TARGETARCH" >&2; exit 1 ;; \
    esac; \
    ln -sf /usr/include/linux "/usr/include/${MUSL_TRIPLE}/linux"; \
    ln -sf /usr/include/asm-generic "/usr/include/${MUSL_TRIPLE}/asm-generic"; \
    ln -sf "/usr/include/${GNU_TRIPLE}/asm" "/usr/include/${MUSL_TRIPLE}/asm"

ENV MUSL_PREFIX=/usr/local/musl
ENV CC=musl-gcc
ENV PKG_CONFIG_PATH=${MUSL_PREFIX}/lib/pkgconfig
ENV PATH=${MUSL_PREFIX}/bin:${PATH}
WORKDIR /build

# 1. OpenSSL — no shared libs, no engines/dso; the same install feeds both
#    xmlsec1's configure and the Rust build's OPENSSL_DIR.
RUN case "$TARGETARCH" in \
      amd64) OSSL_TARGET=linux-x86_64 ;; \
      arm64) OSSL_TARGET=linux-aarch64 ;; \
      *) echo "unsupported TARGETARCH=$TARGETARCH" >&2; exit 1 ;; \
    esac && \
    curl -fsSL "https://github.com/openssl/openssl/releases/download/openssl-${OPENSSL_VER}/openssl-${OPENSSL_VER}.tar.gz" -o openssl.tar.gz && \
    tar xf openssl.tar.gz && cd "openssl-${OPENSSL_VER}" && \
    ./Configure "$OSSL_TARGET" no-shared no-dso no-engine no-tests \
      --prefix="$MUSL_PREFIX" --openssldir="$MUSL_PREFIX/ssl" --libdir=lib && \
    make -j"$(nproc)" && make install_sw && cd .. && rm -rf "openssl-${OPENSSL_VER}" openssl.tar.gz

# 2. libxml2 — minimal: no python/zlib/lzma/http, kills large attack-surface
#    classes we never touch. Static only.
RUN curl -fsSL "https://download.gnome.org/sources/libxml2/${LIBXML2_VER%.*}/libxml2-${LIBXML2_VER}.tar.xz" -o libxml2.tar.xz && \
    tar xf libxml2.tar.xz && cd "libxml2-${LIBXML2_VER}" && \
    ./configure --host="$(${CC} -dumpmachine)" --prefix="$MUSL_PREFIX" \
      --enable-static --disable-shared \
      --without-python --without-zlib --without-lzma --without-http && \
    make -j"$(nproc)" && make install && cd .. && rm -rf "libxml2-${LIBXML2_VER}" libxml2.tar.xz

# 3. xmlsec1 — static, crypto statically bound to OpenSSL (no runtime dlopen),
#    no XSLT (removes the entire XSLT-transform class), OpenSSL only.
RUN curl -fsSL "https://github.com/lsh123/xmlsec/releases/download/${XMLSEC_VER}/xmlsec1-${XMLSEC_VER}.tar.gz" -o xmlsec1.tar.gz && \
    tar xf xmlsec1.tar.gz && cd "xmlsec1-${XMLSEC_VER}" && \
    ./configure --host="$(${CC} -dumpmachine)" --prefix="$MUSL_PREFIX" \
      --enable-static --disable-shared --enable-crypto-dl=no \
      --without-libxslt --without-gnutls --without-nss --with-openssl="$MUSL_PREFIX" \
      --disable-apps --disable-docs && \
    make -j"$(nproc)" && make install && cd .. && rm -rf "xmlsec1-${XMLSEC_VER}" xmlsec1.tar.gz

# 4. Empty stub archives, mirroring upstream musl's install. musl folds
#    libm/libdl/libpthread/librt into libc.a and ships empty stubs so stray
#    `-lm` etc. resolve harmlessly. Our from-source prefix lacks them, and the
#    builder stage links via the host *glibc* cc driver, so xmlsec1/libxml2's
#    pkg-config `-lm` would otherwise fall through to glibc's static libm —
#    undefined refs to _dl_x86_cpu_features/errno at link time.
RUN for lib in m dl pthread rt; do ar rcs "${MUSL_PREFIX}/lib/lib${lib}.a"; done

# ---------------------------------------------------------------------------
# Stage 3: compile the static CLI/server binary against musl (glibc host).
#
# Toolchain notes:
#   * A glibc host so bindgen (samael's build script) can dlopen libclang —
#     impossible from a static-musl host (samael issue #37).
#   * musl-tools provides musl-gcc for the musl target; rusqlite's `bundled`
#     SQLite and ring compile with it via the CC_<target> env below.
#   * The rustls crypto provider must stay pinned to `ring` (never aws-lc-rs)
#     to avoid cmake + arm64 pain; verify with `cargo tree`. The static
#     OpenSSL from stage 2 enters ONLY via samael and coexists with ring
#     (ring 0.17 prefixes its symbols).
# ---------------------------------------------------------------------------
FROM rust:1-bookworm AS builder
ARG TARGETARCH
WORKDIR /src

RUN apt-get update && apt-get install -y --no-install-recommends \
    musl-tools musl-dev linux-libc-dev clang libclang-dev make perl pkg-config \
    && rm -rf /var/lib/apt/lists/*

# Same kernel-header symlink fix as the cdeps stage (rusqlite's bundled SQLite
# and ring compile with musl-gcc here too).
RUN set -eux; \
    case "$TARGETARCH" in \
      amd64) MUSL_TRIPLE=x86_64-linux-musl; GNU_TRIPLE=x86_64-linux-gnu ;; \
      arm64) MUSL_TRIPLE=aarch64-linux-musl; GNU_TRIPLE=aarch64-linux-gnu ;; \
      *) echo "unsupported TARGETARCH=$TARGETARCH" >&2; exit 1 ;; \
    esac; \
    ln -sf /usr/include/linux "/usr/include/${MUSL_TRIPLE}/linux"; \
    ln -sf /usr/include/asm-generic "/usr/include/${MUSL_TRIPLE}/asm-generic"; \
    ln -sf "/usr/include/${GNU_TRIPLE}/asm" "/usr/include/${MUSL_TRIPLE}/asm"

# The musl target for this platform.
RUN case "$TARGETARCH" in \
      amd64) echo x86_64-unknown-linux-musl ;; \
      arm64) echo aarch64-unknown-linux-musl ;; \
      *) echo "unsupported TARGETARCH=$TARGETARCH" >&2; exit 1 ;; \
    esac > /tmp/target && rustup target add "$(cat /tmp/target)"

# The static C libs built in stage 2.
COPY --from=cdeps /usr/local/musl /usr/local/musl

ENV MUSL_PREFIX=/usr/local/musl
ENV PKG_CONFIG_PATH=${MUSL_PREFIX}/lib/pkgconfig
ENV PKG_CONFIG_ALL_STATIC=1
ENV PKG_CONFIG_ALLOW_CROSS=1
ENV OPENSSL_DIR=${MUSL_PREFIX}
ENV OPENSSL_STATIC=1
# xmlsec1-config on PATH so samael's build script finds it and emits static
# link directives (it branches on the absence of XMLSEC_CRYPTO_DYNAMIC_LOADING).
ENV PATH=${MUSL_PREFIX}/bin:${PATH}
# musl-gcc as the cross C compiler for both musl targets (only one is active
# per build, matching the target selected above).
ENV CC_x86_64_unknown_linux_musl=musl-gcc
ENV CC_aarch64_unknown_linux_musl=musl-gcc

# Copy the workspace. Cargo.lock is copied so the release build is reproducible.
COPY Cargo.toml Cargo.lock ./
COPY crates/ ./crates/
# Bring in the compiled UI so rust-embed can bake it into the binary.
COPY --from=web /app/web/dist ./web/dist

# Build only the CLI binary (it pulls in core + cache + server) with SAML on.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    TARGET="$(cat /tmp/target)" && \
    cargo build --release --target "$TARGET" --features saml -p domarinn-cli && \
    cp "target/${TARGET}/release/domarinn" /domarinn && \
    # Fail the build if the binary is not fully static.
    ! ldd /domarinn 2>/dev/null | grep -q '=>' && echo "static OK"

# Empty dir that becomes the runtime image's /data mountpoint. It has to be
# made here: distroless has no shell, so the runtime stage can only COPY it in.
RUN mkdir -p /data

# ---------------------------------------------------------------------------
# Stage 4: runtime. distroless/static has no shell and no libc — a static musl
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
