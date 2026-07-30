#!/usr/bin/env bash
# docs-screenshots.sh — end-to-end docs screenshot pipeline (`mise run screenshots`).
#
# Ensures a real UI build, brings up a local Ollama (unless SKIP_LLM=1), starts
# a throwaway domarinn server, seeds it with real runs of the shipped examples
# (scripts/seed-docs-runs.sh), then drives Playwright to capture the docs'
# reference screenshots into docs/assets/screenshots/.
#
# Env:
#   SKIP_LLM=1        skip Ollama entirely and seed only the offline examples
#                      (implies SEED_OFFLINE_ONLY=1 for the seed script)
#   SEED_EMBEDDINGS=1 also seed example 30 (pulls $OLLAMA_EMBED_MODEL too)
#   KEEP_SERVER=1     skip teardown: leave the seeded server running afterwards
#   SHOTS=<regex>     capture only the shots whose test title matches (passed to
#                      Playwright as --grep). Every other committed PNG is left
#                      exactly as it is — which is what you want when one page
#                      changed and the rest would only churn run ids and dates.
#   OLLAMA_URL, OLLAMA_MODEL, OLLAMA_EMBED_MODEL — see scripts/seed-docs-runs.sh
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

OLLAMA_URL="${OLLAMA_URL:-http://localhost:11434}"
OLLAMA_MODEL="${OLLAMA_MODEL:-qwen3:4b}"
OLLAMA_EMBED_MODEL="${OLLAMA_EMBED_MODEL:-nomic-embed-text}"
SERVER_PORT=8322
SERVER_URL="http://localhost:${SERVER_PORT}"

# The cache the seeded runs actually fill. Defined here rather than in the seed
# script because the server needs it at startup, before any run exists: it is
# mounted read-only as the browsable *local* cache tier, which is what makes
# /cache/entries show real entries. The server tier stays empty on purpose —
# these suites cache to disk, like every default setup — so the cache stats
# page keeps reading zero, which is the thing its docs prose explains.
SEED_CACHE_DIR="${DOMARINN_CACHE_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/domarinn/docs-seed}"
export DOMARINN_CACHE_DIR="$SEED_CACHE_DIR"

BIN="$REPO_ROOT/target/release/domarinn"
DIST_INDEX="$REPO_ROOT/web/dist/index.html"

# --- 1. Ensure a real UI build -----------------------------------------------
#
# The honest check is on web/dist/index.html, not on the compiled binary: that
# file is exactly what crates/domarinn-server/build.rs embeds, and it writes
# the placeholder there itself whenever a `cargo build` runs without the real
# UI present (see build.rs's PLACEHOLDER constant and its "web UI was not
# built into this binary" text). This can't detect a binary that was built
# from a NEWER dist/index.html than the one now on disk, but `mise run build`
# always builds web then cargo together, so that staleness does not happen in
# normal use.
needs_build=0
if [ ! -x "$BIN" ]; then
  needs_build=1
elif [ ! -f "$DIST_INDEX" ] || grep -q "was not built into this binary" "$DIST_INDEX"; then
  needs_build=1
fi
if [ "$needs_build" -eq 1 ]; then
  echo "==> no real UI build found; running 'mise run build'"
  mise run build
else
  echo "==> $BIN already embeds a real UI build"
fi

# --- 2. Ollama (unless skipped) ----------------------------------------------
if [ "${SKIP_LLM:-0}" = "1" ]; then
  echo "==> SKIP_LLM=1: not starting Ollama"
  export SEED_OFFLINE_ONLY="${SEED_OFFLINE_ONLY:-1}"
else
  echo "==> bringing up the ollama compose profile"
  docker compose --profile ollama up -d --wait ollama

  model_present() {
    curl -fsS "$OLLAMA_URL/api/tags" | jq -e --arg m "$1" 'any(.models[]?; .name == $m)' >/dev/null 2>&1
  }

  if model_present "$OLLAMA_MODEL"; then
    echo "==> $OLLAMA_MODEL already pulled"
  else
    echo "==> pulling $OLLAMA_MODEL"
    docker compose --profile ollama exec -T ollama ollama pull "$OLLAMA_MODEL"
  fi

  if [ "${SEED_EMBEDDINGS:-0}" = "1" ]; then
    if model_present "$OLLAMA_EMBED_MODEL"; then
      echo "==> $OLLAMA_EMBED_MODEL already pulled"
    else
      echo "==> pulling $OLLAMA_EMBED_MODEL"
      docker compose --profile ollama exec -T ollama ollama pull "$OLLAMA_EMBED_MODEL"
    fi
  fi
fi

# --- 3. Start a throwaway server ---------------------------------------------
SERVER_DATA_DIR="$(mktemp -d)"
SERVER_LOG="$(mktemp)"

# The tier is refused (with a warning, never a hard failure) if the directory
# does not exist yet, which it will not on a first run.
mkdir -p "$SEED_CACHE_DIR"

echo "==> starting domarinn server on :$SERVER_PORT (data dir: $SERVER_DATA_DIR)"
echo "==> mounting $SEED_CACHE_DIR as the read-only local cache tier"
# Three static tokens: `write` shares the runs, `read` runs the seed's own
# data-at-rest check least-privilege, and `admin` exists only so the seed can
# provision the run-set policy the /sets shots are pictures of. This server is
# thrown away at the end of the script; the tokens are not secrets.
DOMARINN_ADMIN_USER=admin \
  DOMARINN_ADMIN_PASSWORD=screenshots \
  DOMARINN_TOKENS="write:docs-seed-token,read:docs-read-token,admin:docs-admin-token" \
  DOMARINN_LOCAL_CACHE_DIR="$SEED_CACHE_DIR" \
  "$BIN" server --port "$SERVER_PORT" --data-dir "$SERVER_DATA_DIR" \
  >"$SERVER_LOG" 2>&1 &
SERVER_PID=$!

cleanup() {
  if [ "${KEEP_SERVER:-0}" = "1" ]; then
    echo "==> KEEP_SERVER=1: leaving the server (pid $SERVER_PID) and $SERVER_DATA_DIR in place"
  else
    echo "==> stopping the server (pid $SERVER_PID)"
    kill "$SERVER_PID" 2>/dev/null || true
    wait "$SERVER_PID" 2>/dev/null || true
    rm -rf "$SERVER_DATA_DIR" "$SERVER_LOG"
  fi
  if [ "${SKIP_LLM:-0}" != "1" ]; then
    echo "==> ollama is left running; stop it with: docker compose --profile ollama down"
  fi
}
trap cleanup EXIT

# --- 4. Readiness loop --------------------------------------------------------
echo "==> waiting for the server to accept connections"
ready=0
for _ in $(seq 1 60); do
  if "$BIN" healthcheck --port "$SERVER_PORT" >/dev/null 2>&1; then
    ready=1
    break
  fi
  if ! kill -0 "$SERVER_PID" 2>/dev/null; then
    echo "docs-screenshots: the server process exited early; log follows:" >&2
    cat "$SERVER_LOG" >&2
    exit 1
  fi
  sleep 0.5
done
if [ "$ready" -ne 1 ]; then
  echo "docs-screenshots: server never became ready on :$SERVER_PORT; log follows:" >&2
  cat "$SERVER_LOG" >&2
  exit 1
fi

# --- 5. Seed real runs ---------------------------------------------------------
DOMARINN_SERVER_URL="$SERVER_URL" \
  DOMARINN_TOKEN=docs-seed-token \
  DOMARINN_READ_TOKEN=docs-read-token \
  DOMARINN_ADMIN_TOKEN=docs-admin-token \
  OLLAMA_URL="$OLLAMA_URL" \
  OLLAMA_MODEL="$OLLAMA_MODEL" \
  OLLAMA_EMBED_MODEL="$OLLAMA_EMBED_MODEL" \
  SEED_EMBEDDINGS="${SEED_EMBEDDINGS:-0}" \
  "$REPO_ROOT/scripts/seed-docs-runs.sh"

# --- 6. Capture screenshots ----------------------------------------------------
if [ -n "${SHOTS:-}" ]; then
  echo "==> capturing screenshots matching '${SHOTS}'"
fi
# $SHOTS is read by playwright.screenshots.config.ts rather than passed as
# --grep, so that it filters the capture project without also filtering the
# `setup` project this one depends on for its session.
SHOTS="${SHOTS:-}" PLAYWRIGHT_BASE_URL="$SERVER_URL" pnpm -C web run screenshots

# --- 8. Report -----------------------------------------------------------------
echo "==> screenshots written to docs/assets/screenshots/:"
find "$REPO_ROOT/docs/assets/screenshots" -maxdepth 1 -name '*.png' -printf '%f\n' | sort
