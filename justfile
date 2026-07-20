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

# Run the whole test suite.
test:
    cargo test --workspace

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
    cargo run -p measurellm-cli -- schema config > measurellm.schema.json

# Export TypeScript types for the web UI from the Rust DTOs.
#
# TODO: wire this up once ts-rs (or an equivalent) is added to the core crate.
# Intended shape (do not enable until the crate exposes it):
#
#     cargo test -p measurellm-core export_bindings
#     # -> emits web/src/types/*.ts from the #[derive(TS)] DTOs
gen-types:
    @echo "TODO: gen-types is a placeholder until ts-rs export lands in measurellm-core"
    @echo "      (RunResult / config DTOs -> web/src/types). See justfile."

# Build the container image locally (multi-stage; produces the distroless image).
docker-build:
    docker build -t {{image}}:{{tag}} .

# Push the locally built image (requires `docker login ghcr.io`).
docker-push: docker-build
    docker push {{image}}:{{tag}}
