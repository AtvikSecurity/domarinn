#!/usr/bin/env bash
# seed-docs-runs.sh — seed an ALREADY-RUNNING domarinn server with real runs of
# the shipped examples, for the docs screenshot pipeline.
#
# This script never starts or stops a server: scripts/docs-screenshots.sh (or
# `mise run dev`) does that, and hands this one a URL via $DOMARINN_SERVER_URL.
# Every run below is `--share`d to that server so the UI has something real to
# screenshot — a stream of runs, a matrix, a baseline diff, a cache hit, search
# hits, an HTTP provider against a loopback stub, and (unless skipped) several
# runs against a local Ollama endpoint, judge included.
#
# Env:
#   OLLAMA_URL           where Ollama's OpenAI-compatible API lives (default
#                        http://localhost:11434; must be loopback, see below)
#   OLLAMA_MODEL         chat model to use (default qwen3:4b)
#   OLLAMA_EMBED_MODEL   embedding model to use (default nomic-embed-text)
#   DOMARINN_SERVER_URL  the running server to seed (default http://localhost:8322)
#   DOMARINN_TOKEN       a write-scoped bearer token for that server (default
#                        docs-seed-token); used only for the `--share`d runs.
#   DOMARINN_READ_TOKEN  a read-scoped bearer token for the same server (default
#                        docs-read-token); used for the data-at-rest assertion's
#                        GETs, so that check runs least-privilege rather than
#                        reusing the write token it doesn't need.
#   SEED_OFFLINE_ONLY=1  skip the Ollama-backed block entirely (no Ollama needed)
#   SEED_EMBEDDINGS=1    also seed example 30 (needs $OLLAMA_EMBED_MODEL pulled)
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

# --- Environment preamble ---------------------------------------------------
#
# This repo is public. Any ambient DOMARINN_SMOKE_* the caller's shell already
# exports (e.g. from a git-ignored .mise/config.local.toml pointing at a real,
# private endpoint) is tainted for a script whose output — a run's
# config_snapshot — is rendered verbatim by the UI and can end up in a
# committed screenshot. So the vars below are FORCE-exported: derived only
# from $OLLAMA_URL, never inherited from whatever the caller happened to have
# set.

OLLAMA_URL="${OLLAMA_URL:-http://localhost:11434}"

# Localhost allowlist. This is the only thing standing between "seeds
# screenshots" and "captures whatever host a stray OLLAMA_URL points at" — the
# server stores the RESOLVED provider base_url verbatim in a run's
# config_snapshot (crates/domarinn-core/src/runner.rs) and renders it on the
# compare page. An allowlist is safe to commit; naming a real host in a
# denylist here would not be.
ollama_url_host() {
  local host_port="${1#*://}"
  host_port="${host_port%%/*}"
  case "$host_port" in
    \[*)
      host_port="${host_port#\[}"
      printf '%s' "${host_port%%]*}"
      ;;
    *)
      printf '%s' "${host_port%%:*}"
      ;;
  esac
}

ollama_host="$(ollama_url_host "$OLLAMA_URL")"
case "$ollama_host" in
  localhost | 127.0.0.1 | ::1) ;;
  *)
    echo "seed-docs-runs: OLLAMA_URL host '$ollama_host' is not on the localhost allowlist (localhost, 127.0.0.1, [::1])." >&2
    echo "  Port-forward a remote Ollama instead, e.g.: ssh -L 11434:localhost:11434 your-llm-host" >&2
    exit 1
    ;;
esac

OLLAMA_MODEL="${OLLAMA_MODEL:-qwen3:4b}"
OLLAMA_EMBED_MODEL="${OLLAMA_EMBED_MODEL:-nomic-embed-text}"
export DOMARINN_SERVER_URL="${DOMARINN_SERVER_URL:-http://localhost:8322}"
export DOMARINN_TOKEN="${DOMARINN_TOKEN:-docs-seed-token}"
export DOMARINN_READ_TOKEN="${DOMARINN_READ_TOKEN:-docs-read-token}"
export DOMARINN_RUNS_DIR
DOMARINN_RUNS_DIR="$(mktemp -d)" # cleaned by cleanup() below; isolates the local run store

STUB_PID=""
cleanup() {
  if [ -n "$STUB_PID" ]; then
    kill "$STUB_PID" 2>/dev/null || true
    wait "$STUB_PID" 2>/dev/null || true
  fi
  rm -rf "$DOMARINN_RUNS_DIR"
}
trap cleanup EXIT
export DOMARINN_CACHE_DIR="${XDG_CACHE_HOME:-$HOME/.cache}/domarinn/docs-seed" # survives; re-seeds replay as cache hits
export DOMARINN_ACTOR=demo DOMARINN_HOST=docs-seed
export DOMARINN_SMOKE_BASE_URL="${OLLAMA_URL}/v1" DOMARINN_SMOKE_MODEL="$OLLAMA_MODEL" DOMARINN_SMOKE_API_KEY=ollama
export OPENAI_BASE_URL="${OLLAMA_URL}/v1" OPENAI_API_KEY=ollama OPENAI_MODEL="$OLLAMA_MODEL" OPENAI_EMBED_MODEL="$OLLAMA_EMBED_MODEL"

BIN="$REPO_ROOT/target/release/domarinn"
if [ ! -x "$BIN" ]; then
  echo "seed-docs-runs: $BIN not found; run 'mise run build' first" >&2
  exit 1
fi

# run_expect <expected-exit> <example-dir> [extra domarinn-run args...]
run_expect() {
  local expected="$1" dir="$2"
  shift 2
  echo "==> examples/$dir (expect exit $expected) $*"
  local rc=0
  "$BIN" run "examples/$dir" --share "$@" || rc=$?
  if [ "$rc" -ne "$expected" ]; then
    echo "seed-docs-runs: examples/$dir exited $rc, expected $expected" >&2
    exit 1
  fi
}

# run_any <example-dir> [extra domarinn-run args...] — accepts exit 0 or 1.
run_any() {
  local dir="$1"
  shift
  echo "==> examples/$dir (expect exit 0 or 1) $*"
  local rc=0
  "$BIN" run "examples/$dir" --share "$@" || rc=$?
  case "$rc" in
    0 | 1) ;;
    *)
      echo "seed-docs-runs: examples/$dir exited $rc, expected 0 or 1" >&2
      exit 1
      ;;
  esac
}

# --- A loopback stub for the `http` provider example -------------------------
#
# Example 36 is offline in the sense that matters here — it calls no model — but
# it does speak HTTP, and its `url` defaults to a public example host this
# pipeline must never reach. The suite redirects with
# `${env:ORDERS_API_URL:-…}`, exactly as the CI harness does
# (crates/domarinn-cli/tests/examples/table.rs, `Env::StubBase`), so point it at
# a loopback stub that answers the one body its two `output_expr`s read. The
# stub's URL is what lands in the run's config_snapshot, which is why it has to
# be loopback and not merely unreachable.
STUB_PORT="${STUB_PORT:-8323}"
export ORDERS_API_URL="http://127.0.0.1:${STUB_PORT}"

echo "==> starting the loopback HTTP stub on :$STUB_PORT (for examples/36)"
python3 - "$STUB_PORT" <<'PY' &
import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

# The same shape crates/domarinn-cli/tests/examples/stubs.rs serves: a reply
# string containing "shipped"/"Tuesday"/"Thursday" and a numeric confidence of
# 0.93, which are what example 36's two providers assert on.
BODY = json.dumps(
    {
        "request_id": "req_stub",
        "result": {
            "reply": "Your order 1042 shipped on Tuesday and arrives Thursday.",
            "confidence": 0.93,
        },
    }
).encode()


class Handler(BaseHTTPRequestHandler):
    def do_POST(self):  # noqa: N802 (BaseHTTPRequestHandler's naming)
        length = int(self.headers.get("content-length") or 0)
        self.rfile.read(length)
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(BODY)))
        self.end_headers()
        self.wfile.write(BODY)

    def log_message(self, *_args):
        pass  # the seed log is noisy enough


ThreadingHTTPServer(("127.0.0.1", int(sys.argv[1])), Handler).serve_forever()
PY
STUB_PID=$!

stub_ready=0
for _ in $(seq 1 40); do
  if curl -fsS -X POST -H 'content-type: application/json' -d '{}' \
    "$ORDERS_API_URL" >/dev/null 2>&1; then
    stub_ready=1
    break
  fi
  sleep 0.25
done
if [ "$stub_ready" -ne 1 ]; then
  echo "seed-docs-runs: the loopback HTTP stub never came up on :$STUB_PORT" >&2
  exit 1
fi

echo "==> seeding offline exec examples (no model calls)"
run_expect 0 01-hello-eval
run_expect 0 01-hello-eval
run_expect 0 03-deterministic-asserts
run_expect 0 04-json-output
run_expect 0 08-matrix-sweeps
run_expect 0 08-matrix-sweeps
run_expect 0 15-tool-call-asserts
run_expect 0 16-tags-and-filters
run_expect 1 18-failing-gate
run_expect 3 19-errors-and-retries
run_expect 0 23-repeat-and-confidence --repeat 3
run_expect 0 24-baselines-and-diff
run_any 24-baselines-and-diff --against latest
run_expect 0 34-multi-turn-conversation
run_expect 0 36-http-output-expr # the one HTTP-provider run; answered by the stub above
run_expect 0 37-exec-provider-bash
run_expect 0 39-import-promptfoo

if [ "${SEED_OFFLINE_ONLY:-0}" = "1" ]; then
  echo "==> SEED_OFFLINE_ONLY=1: skipping the Ollama-backed block"
else
  echo "==> seeding live-endpoint examples against $OLLAMA_URL"
  run_expect 0 32-live-endpoint-smoke
  run_expect 0 32-live-endpoint-smoke # second run: shows cache hits
  run_expect 0 26-openai-provider
  # The two graded suites, judged by the same local endpoint via OPENAI_*.
  # `run_any`, not `run_expect 0`: a rubric's verdict is the local judge's
  # opinion, and a small model that scores one case zero must leave the docs
  # with a real failing verdict to show rather than aborting the seed.
  run_any 33-openai-grader-rubric
  run_any 38-annotated-reference-suite
  if [ "${SEED_EMBEDDINGS:-0}" = "1" ]; then
    # Also `run_any`: example 30's `threshold: 0.85` is a cosine, and cosines
    # are not comparable across embedding models — the very thing that example
    # says. nomic-embed-text scores its paraphrase around 0.76, so this run
    # fails here and would pass against OpenAI's embedding model. That is real
    # data about a local model, not a broken seed.
    run_any 30-similar-embeddings
  fi
fi

# --- Data-at-rest hygiene assertion -----------------------------------------
#
# Every run just shared carries a config_snapshot the server returns verbatim
# and the UI renders on the compare page. Prove none of them named anything
# other than loopback before this script hands off to the capture pipeline.
#
# Read-only, so this authenticates with $DOMARINN_READ_TOKEN rather than the
# write-scoped $DOMARINN_TOKEN used above for `--share`: the write token would
# work too (write subsumes read), but the assertion needs no write access, and
# reusing it here would leave the read token dead configuration.
echo "==> checking every seeded run's config_snapshot for non-localhost endpoints"
if ! command -v jq >/dev/null 2>&1; then
  echo "seed-docs-runs: jq is required for the data-at-rest check (pinned in .mise/config.toml)" >&2
  exit 1
fi

auth_header="Authorization: Bearer $DOMARINN_READ_TOKEN"
runs_json="$(curl -fsS -H "$auth_header" "$DOMARINN_SERVER_URL/api/v1/runs?limit=100")"
run_ids="$(printf '%s' "$runs_json" | jq -r '.runs[].id')"

checked=0
violations=0
while IFS= read -r run_id; do
  [ -n "$run_id" ] || continue
  config_json="$(curl -fsS -H "$auth_header" "$DOMARINN_SERVER_URL/api/v1/runs/$run_id/config")"
  # Both key names, because both name an endpoint: the model providers resolve
  # `base_url`, and `type: http` resolves `url`. Checking only the first would
  # have waved example 36 through to a public host.
  bad="$(printf '%s' "$config_json" | jq -r '
    [.. | objects | (select(has("base_url")) | .base_url), (select(has("url")) | .url)]
    | .[]
    | select(type == "string")
    | select((startswith("http://localhost") or startswith("http://127.0.0.1")) | not)
  ')"
  if [ -n "$bad" ]; then
    while IFS= read -r value; do
      echo "seed-docs-runs: run $run_id has a non-localhost base_url: $value" >&2
    done <<<"$bad"
    violations=$((violations + 1))
  fi
  checked=$((checked + 1))
done <<<"$run_ids"

if [ "$violations" -gt 0 ]; then
  echo "seed-docs-runs: $violations of $checked seeded run(s) failed the data-at-rest check" >&2
  exit 1
fi
echo "    $checked run(s) checked, all base_url values are localhost"
