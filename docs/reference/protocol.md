# The domarinn exec protocol (v1)

This is the external contract for writing **providers**, **asserts**, and **test generators** in any language. If your program can read JSON from stdin and write JSON to stdout, it can plug into domarinn — no Rust, no SDK, no linking required.

> The canonical shapes live in
> [`crates/domarinn-protocol/src/lib.rs`](https://github.com/AtvikSecurity/domarinn/blob/main/crates/domarinn-protocol/src/lib.rs).
> This document mirrors that file; if the two ever disagree, the Rust source
> wins.

## One protocol, three kinds

A single wire protocol carries three kinds of request, distinguished by the envelope's `kind` field:

| `kind`           | Program role      | Answers the question           |
|------------------|-------------------|--------------------------------|
| `provider`       | System under test | "What does the model output?"  |
| `assert`         | Grader            | "Did the output pass?"         |
| `generate_tests` | Test source       | "Which tests should run?"      |

## Transport: one-shot stdin -> stdout

Version 1 is **one-shot**:

1. domarinn spawns your program (an `exec` command from the suite config).
2. It writes **exactly one** JSON request document to your **stdin**, then **closes stdin** (EOF).
3. Your program writes **exactly one** JSON response document to **stdout** and exits.

Rules:

- **stdout is for the JSON response only.** Write logs, diagnostics, and progress to **stderr**. A non-JSON stdout is a protocol violation.
- The environment variable `DOMARINN_PROTOCOL` is set to the integer protocol version (`1`). Use it to detect the version if you support more than one.
- **Exit code = infrastructure signal, not grading.** Exit `0` when you produced a valid response — *even if a provider errored or an assert failed*; that outcome belongs in the JSON body. A **non-zero exit** means your program itself broke (crash, could not reach an upstream API, bad input) and domarinn treats it as an infrastructure error for that call.
- A per-provider `timeout_ms` (from the suite) bounds each call; exceeding it is an infrastructure error.

## The envelope

Every request is a JSON object with a top-level `domarinn` key:

```json
{
  "domarinn": { "protocol": 1, "kind": "provider" }
}
```

- `protocol` (integer) — the protocol version; `1` today.
- `kind` (string) — one of `provider`, `assert`, `generate_tests` (snake_case).

The envelope makes the protocol evolvable: future versions bump `protocol` and may add fields. Unknown fields should be ignored by well-behaved programs.

---

## Kind: `provider`

Runs the system under test. Prompts are **optional** — when a suite has no prompts, `prompt` is omitted and your provider works from `vars` alone (the "self-input" case).

### Request (domarinn -> your stdin)

```json
{
  "domarinn": { "protocol": 1, "kind": "provider" },
  "prompt": { "role": "user", "content": "..." },
  "vars": { "user_input": "hello world" },
  "params": { "temperature": 0.0 },
  "test": { "id": "greeting/basic", "tags": ["smoke"] },
  "tools": [
    {
      "name": "get_weather",
      "description": "Look up the current weather",
      "input_schema": { "type": "object", "properties": { "city": { "type": "string" } } }
    }
  ]
}
```

| Field    | Type            | Notes |
|----------|-----------------|-------|
| `prompt` | any JSON        | **Optional.** The rendered prompt. Absent when the suite has no prompts. |
| `vars`   | any JSON object | Template variables for this test. Defaults to `{}`. |
| `params` | any JSON object | Provider parameters from the suite (model, temperature, ...). Defaults to `{}`. |
| `test`   | object          | `{ "id": string, "tags": string[] }`. `tags` defaults to `[]`. Correlation metadata: it is sent, but [stripped out of the keyed request](../concepts/caching.md#what-is-in-the-key), so renaming a test does not re-run it. |
| `tools`  | array           | Optional. Tools the suite declared. **Absent when it declared none**, so a tool-free request is exactly what it always was. See [Tools](#tools). |

### Response (your stdout -> domarinn)

Only `output` is required:

```json
{
  "output": "hello, world",
  "usage": { "input_tokens": 12, "output_tokens": 4 },
  "cost_usd": 0.0001,
  "metadata": { "model": "my-svc-v3" }
}
```

| Field | Type | Notes |
|---|---|---|
| `output` | any JSON | **Required.** The output to assert against (string or structured JSON). |
| `usage` | object | Optional. See [Usage](#usage). |
| `cost_usd` | number | Optional. Dollar cost of the call. Wins over any configured rate — you know what you actually spent. |
| `error` | object | Optional. See [Errors](#errors). Report an upstream failure here **and still exit 0**. |
| `metadata` | any JSON | Optional. Free-form; surfaced in results. |
| `stop_reason` | string | Optional. The vendor's finish reason, verbatim (`end_turn`, `length`, `refusal`, …). Never validated against a list. |
| `empty_reason` | string | Optional. Why `output` has nothing gradeable in it. See [Empty outputs](#empty-outputs). |
| `reasoning` | string | Optional. The model's reasoning/thinking text, when you can expose it. |
| `model` | string | Optional. The model you *actually* used. Response metadata, never part of a cache key. |
| `tool_calls` | array | Optional. The tool calls the model decided to make, in order. See [Tools](#tools). |

#### Usage

| Field | Notes |
|---|---|
| `input_tokens` | Tokens billed at the full input rate. Defaults to `0`. |
| `output_tokens` | Defaults to `0`. |
| `cache_read_tokens` | Optional. Input served from a provider-side prompt cache. |
| `cache_write_tokens` | Optional. Input written *into* that cache. |
| `cache_write_1h_tokens` | Optional. The subset of `cache_write_tokens` at a longer TTL. Absent means "all at the default TTL". |

`input_tokens` **excludes** the cache counters, so the fields sum. If your upstream reports an inclusive total (OpenAI's `prompt_tokens` includes `cached_tokens`), subtract the cached span out before reporting — otherwise it is billed at the discounted rate *and* the full one.

#### Errors

| Field | Notes |
|---|---|
| `message` | **Required.** Prose, for a human. |
| `retriable` | Defaults to `false`. Whether a retry could plausibly help. |
| `details` | Optional. The machine-readable half of `message`. Surfaced on the stored case as `error_details`. |
| `class` | Optional. What kind of failure this was, using domarinn's vocabulary — `provider_auth`, `provider_rate_limit`, `provider_timeout`, `provider_unavailable`, `provider_protocol`. Defaults to `exec_failed`. |
| `retry_after_ms` | Optional. A `Retry-After` you received. Only meaningful with `retriable: true`. |

Naming a `class` is worth doing: without it every failure from every exec provider is indistinguishable, so a rejected credential looks exactly like a crash. Unrecognized values are kept verbatim rather than rejected.

#### Empty outputs

A blank `output` is a *successful* call, so nothing upstream raises — and every assertion then scores zero for a reason that has nothing to do with the prompt. Set `empty_reason` when you know why:

| Value | Means |
|---|---|
| `refusal` | The model declined. Genuine model behavior, not a harness fault. |
| `truncated` | Cut off before any text. Fix: raise `max_tokens`. |
| `content_filter` | A provider-side safety filter removed the content. |
| `tool_use_only` | The model called a tool and said nothing else. |
| `thinking_only` | It reasoned but never emitted a final message. |
| `no_content_blocks`, `empty_body`, `blank` | Protocol-shaped faults, or a genuinely blank answer. |

An unrecognized value is carried through verbatim and is **never** an error — the list grows at model-release cadence, with no domarinn release in the loop. If you report `stop_reason` but not `empty_reason`, domarinn derives one when (and only when) the output really is blank.

To signal a recoverable upstream failure (e.g. a rate limit) without crashing:

```json
{ "output": null, "error": { "message": "429 from upstream", "retriable": true } }
```

---

## Tools

domarinn can hand your provider a set of tool declarations and grade what the model decided to do with them.

**It never runs a tool.** There is no agent loop, no result fed back, no second turn. What an eval wants from a tool-using model is the *decision* — did it reach for `get_weather` with the right city, and did it stay away from `delete_account` — and that decision is fully observable in one turn. Executing the tool would make domarinn part of the system under test.

### Offering them

`tools` arrives on the request when the suite declared any, in Anthropic's shape (`input_schema` is a JSON Schema). Offer them to your model however your upstream expects; an OpenAI-compatible endpoint wants each one wrapped as `{"type": "function", "function": {name, description, parameters}}`.

The key is absent, not empty, when the suite declared no tools — so a provider that ignores this field is unaffected, and so is every cache entry keyed on a tool-free request.

### Reporting the decision

```json
{
  "output": "",
  "empty_reason": "tool_use_only",
  "tool_calls": [
    { "id": "call_1", "name": "get_weather", "arguments": { "city": "Oslo" } }
  ]
}
```

| Field | Type | Notes |
|---|---|---|
| `id` | string | Optional. Your upstream's call id, so a multi-call response stays attributable. Never interpreted. |
| `name` | string | **Required.** A call with no name is dropped: an assertion must not match something that cannot be attributed. |
| `arguments` | any JSON | The **decoded** arguments object. |

Two rules worth stating plainly:

- **`arguments` is decoded, not a string.** OpenAI-compatible endpoints send `function.arguments` as a JSON string; parse it before reporting. Forwarding the string hands every assertion a parsing problem instead of an argument.
- **Report calls even when there is also text.** A case whose right answer is a tool call has no gradeable prose, and returning `""` alone scores it zero against every assertion for a reason unrelated to the prompt. Set `empty_reason: "tool_use_only"` alongside the calls when that is what happened.

Suite-side, these are graded by [`tool-call` assertions](assertions.md#tool-call).

## Writing a provider in Rust

The wire types live in `domarinn-protocol` — a crate whose only dependencies are `serde` and `serde_json`, enforced by a test. It is deliberately separate from `domarinn-types` (the *run document* contract, which carries a schema generator and a TypeScript exporter): someone writing a provider should not inherit either.

```toml
[dependencies]
domarinn-protocol = { git = "https://github.com/AtvikSecurity/domarinn", tag = "0.3.0" }
```

```rust
use domarinn_protocol::{ProviderReq, ProviderResp};

let req: ProviderReq = serde_json::from_reader(std::io::stdin())?;
let resp = ProviderResp {
    output: serde_json::Value::String(answer),
    ..Default::default()
};
serde_json::to_writer(std::io::stdout(), &resp)?;
```

`Default` is derived precisely so this stays correct as optional fields are added.

## Working directory

Every child — provider, assertion, and generator — is spawned with its working directory set to the **suite's** directory, not wherever the CLI was invoked. A generator resolving `datasets/*.yaml` therefore resolves it relative to the suite file, which is almost always what you want and is not otherwise discoverable.

## Kind: `assert`

A custom grader. Receives the provider's output plus context and returns a `GradingResult`-shaped verdict.

### Request

```json
{
  "domarinn": { "protocol": 1, "kind": "assert" },
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

A failing assert is a **normal result** (`"pass": false`), not an error — exit `0`. Reserve non-zero exits for the grader itself breaking.

---

## Kind: `generate_tests`

Produces tests programmatically (e.g. from a dataset, an API, or an LLM).

### Request

```json
{
  "domarinn": { "protocol": 1, "kind": "generate_tests" },
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

**JSONL form** — one test JSON object per line (no wrapper). Convenient for streaming large generators.

Each emitted test object follows the suite's test schema (see `domarinn schema config`).

---

## Writing a provider

Two, in two languages and at two levels of detail. They also show the two equivalent ways to check the wire version: Bash reads the `DOMARINN_PROTOCOL` environment variable, Python reads `domarinn.protocol` out of the request. domarinn sets both on every call, so either one is enough.

### Bash

The shipped script in full, comments and all — [example 37](../examples/your-own-system.md#example-37--the-exec-protocol-in-bash). It reads `DOMARINN_PROTOCOL`, extracts both `vars.user_input` and `test.id` with `jq`, and reports `usage`:

```sh
--8<-- "examples/37-exec-provider-bash/provider.sh"
```

### Python

Stripped to the four lines that carry the contract:

```python
#!/usr/bin/env python3
import json, sys

req = json.load(sys.stdin)                    # one request
assert req["domarinn"]["protocol"] == 1
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

A test may also set its own `cache_salt`. That one is purely a cache-keying concern and is deliberately **not** part of the wire payload — do not look for it in the request. Use it when your program resolves content domarinn cannot see (its own prompt files, say), so that editing that content busts only the affected cases. See [caching.md](../concepts/caching.md#per-case-salts).

## Versioning

- `protocol: 1` is the current, stable wire version.
- New versions bump the integer and are negotiated via the `DOMARINN_PROTOCOL` env var and the envelope field. Programs should read the version rather than assume it, and ignore unknown fields for forward compatibility.
- **The envelope is deliberately part of the cache key.** It is genuinely sent, and a child may answer a v2 request differently, so it stays in the keyed request. That makes a version bump a **cache flag day**: every `exec` entry in every store is re-keyed at once. A test in `cache_key.rs` asserts the current version so the decision is made on purpose, with the protocol-1 shape frozen as a legacy generation first. See [Upgrading to 0.5](../concepts/caching.md#upgrading-to-05).
