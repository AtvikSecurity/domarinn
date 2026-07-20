# measurellm task runner. Install `just`: https://github.com/casey/just
#
#   just            # list recipes
#   just build      # build UI + release binary
#   just dev        # run the server (see note about the UI dev server)

# Image reference used by `docker-build` / `docker-push`.
image := "ghcr.io/perfectra1n/measurellm"
tag := "dev"

# Show the available recipes.
default:
    @just --list

# Full release build: compile the web UI, then the Rust workspace in release
# mode. The server crate embeds web/dist, so the UI must build first.
build:
    pnpm -C web build
    cargo build --release

# Run the results server + API on :8321 for local development.
#
# NOTE: this serves the *embedded* UI (whatever was last built into the
# binary). For live UI development run the Vite dev server in a second shell:
#
#     pnpm -C web dev
#
# and point it at this server's API.
dev:
    cargo run -p measurellm-cli -- server

# Run the Rust test suite.
test:
    cargo test --workspace

# Run every test: Rust workspace tests + the web unit tests (vitest).
test-all: test
    pnpm -C web test

# Run the web end-to-end tests (Playwright). Builds the mock UI and previews it
# via the config's webServer, so no Rust backend is required.
e2e:
    pnpm -C web test:e2e

# Lint: clippy as errors + a formatting check. Mirrors CI.
lint:
    cargo clippy --workspace --all-targets -- -D warnings
    cargo fmt --check

# Auto-format the workspace.
fmt:
    cargo fmt

# Regenerate the config JSON Schema that editors and CI consume. Keep the
# result committed; CI fails if it drifts (see .github/workflows/ci.yml).
schema:
    cargo run -q -p measurellm-cli -- schema config > measurellm.schema.json

# Export TypeScript DTOs for the web UI from the Rust result/diff types. This
# directory is the web type source of truth; keep it committed. CI regenerates
# it and fails on drift (see the gen-types-check job in ci.yml).
gen-types:
    cargo run -q -p measurellm-cli -- gen-types web/src/api/generated

# Build the container image locally (multi-stage; produces the distroless image).
docker-build:
    docker build -t {{image}}:{{tag}} .

# Push the locally built image (requires `docker login ghcr.io`).
docker-push: docker-build
    docker push {{image}}:{{tag}}
