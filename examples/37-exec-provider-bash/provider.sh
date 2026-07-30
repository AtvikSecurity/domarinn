#!/usr/bin/env bash
# The exec protocol needs no SDK: read one JSON request from stdin, write one
# JSON response to stdout. This is the same contract ../echo-provider.py
# speaks, in bash + jq instead of Python.
#
# Exit 0 means "I produced a response" — even one describing a failure; a
# non-zero exit means THIS SCRIPT broke, which domarinn reports as an
# infrastructure error rather than a graded result. `set -e` below is what
# makes an unhandled jq/bash error surface as that non-zero exit instead of
# limping on with a half-built reply.
set -euo pipefail

# domarinn sets this to the wire version it is speaking, so a child that
# supports more than one can branch on it. Today there is only "1".
[[ "${DOMARINN_PROTOCOL:-}" == "1" ]] || {
  echo "provider.sh: unsupported protocol '${DOMARINN_PROTOCOL:-<unset>}'" >&2
  exit 1
}

# Exactly one JSON document arrives on stdin, then domarinn closes it — `cat`
# sees a clean EOF rather than blocking for more input.
request="$(cat)"

# `-r` unwraps the JSON string to raw text so it can be interpolated below
# without its quotes. `// ""` is the fallback for a suite that never sets
# `user_input` — an exec provider is not guaranteed any particular var.
user_input="$(jq -r '.vars.user_input // ""' <<<"$request")"
test_id="$(jq -r '.test.id' <<<"$request")"

# Exactly one JSON object on stdout; `output` is the only field domarinn
# requires. `-n` builds it from `null` rather than re-reading `request`, so
# nothing already consumed leaks into the reply by accident.
jq -cn --arg out "case ${test_id}: ${user_input}" \
  '{output: $out, usage: {input_tokens: 1, output_tokens: 1}}'
