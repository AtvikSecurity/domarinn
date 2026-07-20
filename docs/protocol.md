# The measurellm exec protocol (v1)

This is the external contract for writing **providers**, **asserts**, and
**test generators** in any language. If your program can read JSON from stdin
and write JSON to stdout, it can plug into measurellm — no Rust, no SDK, no
linking required.

> The canonical shapes live in
> [`crates/measurellm-core/src/exec_protocol.rs`](../crates/measurellm-core/src/exec_protocol.rs).
> This document mirrors that file; if the two ever disagree, the Rust source
> wins.

## One protocol, three kinds

A single wire protocol carries three kinds of request, distinguished by the
envelope's `kind` field:

| `kind`           | Program role      | Answers the question           |
|------------------|-------------------|--------------------------------|
| `provider`       | System under test | "What does the model output?"  |
| `assert`         | Grader            | "Did the output pass?"         |
| `generate_tests` | Test source       | "What test cases should run?"  |

## Transport: one-shot stdin -> stdout

Version 1 is **one-shot**:

1. measurellm spawns your program (an `exec` command from the suite config).
2. It writes **exactly one** JSON request document to your **stdin**, then
   **closes stdin** (EOF).
3. Your program writes **exactly one** JSON response document to **stdout** and
   exits.

Rules:

- **stdout is for the JSON response only.** Write logs, diagnostics, and
  progress to **stderr**. A non-JSON stdout is a protocol violation.
- The environment variable `MEASURELLM_PROTOCOL` is set to the integer protocol
  version (`1`). Use it to detect the version if you support more than one.
- **Exit code = infrastructure signal, not grading.** Exit `0` when you
  produced a valid response — *even if a provider errored or an assert failed*;
  that outcome belongs in the JSON body. A **non-zero exit** means your program
  itself broke (crash, could not reach an upstream API, bad input) and
  measurellm treats it as an infrastructure error for that call.
- A per-provider `timeout_ms` (from the suite) bounds each call; exceeding it is
  an infrastructure error.

## The envelope

Every request is a JSON object with a top-level `measurellm` key:

```json
{
  "measurellm": { "protocol": 1, "kind": "provider" }
}
```

- `protocol` (integer) — the protocol version; `1` today.
- `kind` (string) — one of `provider`, `assert`, `generate_tests`
  (snake_case).

The envelope makes the protocol evolvable: future versions bump `protocol` and
may add fields. Unknown fields should be ignored by well-behaved programs.

---

## Kind: `provider`

Runs the system under test. Prompts are **optional** — when a suite has no
prompts, `prompt` is omitted and your provider works from `vars` alone (the
"self-input" case).

### Request (measurellm -> your stdin)

```json
{
  "measurellm": { "protocol": 1, "kind": "provider" },
  "prompt": { "role": "user", "content": "..." },
  "vars": { "user_input": "hello world" },
  "params": { "temperature": 0.0 },
  "test": { "id": "greeting/basic", "tags": ["smoke"] }
}
```

| Field    | Type            | Notes |
|----------|-----------------|-------|
| `prompt` | any JSON        | **Optional.** The rendered prompt. Absent when the suite has no prompts. |
| `vars`   | any JSON object | Template variables for this test case. Defaults to `{}`. |
| `params` | any JSON object | Provider parameters from the suite (model, temperature, ...). Defaults to `{}`. |
| `test`   | object          | `{ "id": string, "tags": string[] }`. `tags` defaults to `[]`. |

### Response (your stdout -> measurellm)

Only `output` is required:

```json
{
  "output": "hello, world",
  "usage": { "input_tokens": 12, "output_tokens": 4 },
  "cost_usd": 0.0001,
  "metadata": { "model": "my-svc-v3" }
}
```

| Field      | Type            | Notes |
|------------|-----------------|-------|
| `output`   | any JSON        | **Required.** The output to assert against (string or structured JSON). |
| `usage`    | object          | Optional. `{ "input_tokens": u64, "output_tokens": u64 }` (each defaults to `0`). |
| `cost_usd` | number          | Optional. Dollar cost of the call. |
| `error`    | object          | Optional. `{ "message": string, "retriable": bool }`. Report an upstream failure here **and still exit 0**. |
| `metadata` | any JSON        | Optional. Free-form; surfaced in results. |

To signal a recoverable upstream failure (e.g. a rate limit) without crashing:

```json
{ "output": null, "error": { "message": "429 from upstream", "retriable": true } }
```

---

## Kind: `assert`

A custom grader. Receives the provider's output plus context and returns a
`GradingResult`-shaped verdict.

### Request

```json
{
  "measurellm": { "protocol": 1, "kind": "assert" },
  "output": "hello, world",
  "test": { "id": "greeting/basic", "tags": ["smoke"] },
  "prompt": { "role": "user", "content": "..." },
  "provider": { "id": "renderer" },
  "config": { "value": "hello" }
}
```

| Field      | Type            | Notes |
|------------|-----------------|-------|
| `output`   | any JSON        | **Required.** The provider output to grade. |
| `test`     | object          | `{ "id", "tags" }`, as above. |
| `prompt`   | any JSON        | **Optional.** The prompt that produced the output. |
| `provider` | object          | `{ "id": string }` — which provider produced the output. |
| `config`   | any JSON        | The assertion's own config block from the suite. Defaults to `{}`. |

### Response

```json
{
  "pass": true,
  "score": 1.0,
  "reason": "output contained the expected greeting",
  "details": { "matched": "hello" }
}
```

| Field     | Type    | Notes |
|-----------|---------|-------|
| `pass`    | boolean | **Required.** Did the assertion pass? |
| `score`   | number  | Optional. `0.0`-`1.0` graded score. |
| `reason`  | string  | Optional. Human-readable explanation (shown in results). |
| `details` | any JSON| Optional. Structured evidence. |

A failing assert is a **normal result** (`"pass": false`), not an error — exit
`0`. Reserve non-zero exits for the grader itself breaking.

---

## Kind: `generate_tests`

Produces test cases programmatically (e.g. from a dataset, an API, or an LLM).

### Request

```json
{
  "measurellm": { "protocol": 1, "kind": "generate_tests" },
  "config": { "n": 50, "source": "cases.csv" }
}
```

| Field    | Type     | Notes |
|----------|----------|-------|
| `config` | any JSON | The generator's config block from the suite. Defaults to `{}`. |

### Response

Two accepted forms:

**Object form** — one JSON object with a `tests` array:

```json
{
  "tests": [
    { "id": "gen/1", "vars": { "user_input": "..." }, "assert": [ { "type": "contains", "value": "..." } ] },
    { "id": "gen/2", "vars": { "user_input": "..." } }
  ]
}
```

**JSONL form** — one test case JSON object per line (no wrapper). Convenient for
streaming large generators.

Each emitted test object follows the suite's test-case schema (see
`measurellm schema config`).

---

## Writing a provider: minimal examples

### Bash

```bash
#!/usr/bin/env bash
# Echo the rendered user_input back as the output. Exit 0 == "I ran".
set -euo pipefail
req="$(cat)"                                  # read the whole request
input="$(printf '%s' "$req" | jq -r '.vars.user_input // ""')"
jq -cn --arg out "$input" '{ output: $out }'  # one JSON line to stdout
```

### Python

```python
#!/usr/bin/env python3
import json, sys

req = json.load(sys.stdin)                    # one request
assert req["measurellm"]["protocol"] == 1
out = req.get("vars", {}).get("user_input", "")
json.dump({"output": out}, sys.stdout)        # one response; exit 0
```

### Wiring it into a suite

```yaml
providers:
  - id: my-service
    type: exec
    command: ["./provider.py"]
    timeout_ms: 30000
    # Bump when the program changes so a rebuilt binary doesn't serve stale cache.
    cache_salt: "v1"
```

## Versioning

- `protocol: 1` is the current, stable wire version.
- New versions bump the integer and are negotiated via the `MEASURELLM_PROTOCOL`
  env var and the envelope field. Programs should read the version rather than
  assume it, and ignore unknown fields for forward compatibility.
