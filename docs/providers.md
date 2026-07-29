# Providers

A **provider** is a system under test: the thing domarinn sends inputs to and grades the output of. Every suite lists at least one under `providers:`, and the run matrix is `providers × prompts × tests × repeats`.

A provider is selected by its `type`. Five types exist:

| `type`       | What it is | System under test? |
|--------------|------------|--------------------|
| `exec`       | An external command speaking the exec JSON protocol (any language). | yes |
| `anthropic`  | Native Anthropic Messages API client. | yes |
| `openai`     | OpenAI-compatible chat-completions client (OpenAI + any compatible gateway). | yes |
| `http`       | An arbitrary templated HTTP endpoint. | yes |
| `embeddings` | An OpenAI-compatible `/embeddings` client. | no — it powers the [`similar`](./assertions.md#similar) assertion |

Every provider has an `id` (used in results and cache keys) and an optional `label`. The remaining fields depend on `type`.

> Source of truth: `crates/domarinn-core/src/exec_provider.rs`,
> `anthropic.rs`, `openai.rs`, `http_provider.rs`, `embeddings.rs`, the shared
> networking in `net.rs`, and the `ProviderKind` schema in `config.rs`.

### Environment-driven config

Any string in a provider's configuration may contain a `${env:VAR}` placeholder, resolved once at load time — handy for a per-developer endpoint or a per-environment gateway that shouldn't be committed:

```yaml
providers:
  - id: gateway
    type: openai
    model: "${env:LLM_MODEL}"
    base_url: "${env:LLM_BASE_URL:-https://api.openai.com/v1}"
    api_key_env: LLM_API_KEY
```

An unset `${env:VAR}` with no `:-default` is a hard load error that names the field and the variable; `$${env:VAR}` escapes to a literal. This resolves the *endpoint*, not the *secret* — API keys are still read at call time from the variable named by `api_key_env`. The same interpolation covers a `grader`'s `provider` block and `cache.s3`, but never test `vars`. See [Environment interpolation](./configuration.md#environment-interpolation-envvar) for the full rules.

---

## `exec` — the flagship

An `exec` provider shells out to a command that speaks the **exec JSON protocol**. If your program can read JSON from stdin and write JSON to stdout, it is a provider — no Rust, no SDK.

| Field        | Type                | Default    | Meaning |
|--------------|---------------------|------------|---------|
| `command`    | `[string]`          | –          | The command and its argv. |
| `env`        | `{string: string}`  | `{}`       | Extra environment variables for the child. |
| `timeout_ms` | integer             | `60000`    | Per-call timeout in milliseconds. |
| `cache_salt` | string              | *(none)*   | Version pin for the program — set it when a rebuild should discard cached answers. See below. |

### Wire behavior

For each call the provider writes one `provider` request to the child's stdin and closes it, then reads one JSON response from stdout:

- **Request** (domarinn → child stdin): `{ "domarinn": {"protocol": 1, "kind": "provider"}, "prompt"?, "vars", "params", "test": {"id", "tags"} }`. `prompt` is **null / omitted** when the suite has no prompts (the "self-input" case) — the provider works from `vars` alone. A text prompt is sent as `{ "text": "…" }`; a chat prompt as `{ "messages": [...] }`.
- **Response** (child stdout → domarinn): `output` is the only required field; see [the protocol reference](./protocol.md#response) for the full set. A string `output` becomes text; any other JSON becomes a structured output. `usage` fills token counts, `cost_usd` feeds the [`cost`](./assertions.md#budget-assertions-cost-latency-tokens) assertion, and `metadata` is retained as the raw payload.
- Worth reporting even though all of it is optional: `empty_reason` (so a refusal is diagnosed instead of scoring zero against every assertion), `error.class` (so a rejected credential is distinguishable from a crash), `error.details` (structured diagnostics that survive to the stored case), and `model` (so an alias that silently repointed is visible).

The child **always** receives `DOMARINN_PROTOCOL=1` in its environment, plus your `env`. The full wire contract, exit-code rules, and minimal Bash/Python examples live in **[protocol.md](./protocol.md)**.

### Caching, and when you need `cache_salt`

`exec` providers are **cached by default**. The key names what will answer — `command`, `env`, and any `cache_salt` — and hashes what is asked. It says nothing about the program's *bytes*, so an entry written on one machine is reusable on every other: a fresh clone, a different checkout path, a rebuilt binary and a different working directory all key identically.

The price of that is that domarinn cannot tell one build of your program from the next, so **set `cache_salt` when a rebuild should discard the old answers**:

- **A program you rebuild** — a compiled binary, or a script you are actively editing between runs. Use a commit SHA, a release tag, or `"$digest: src/**/*.rs"`. In CI this matters most, and a SHA is more honest than hashing the artifact, since two runners compiling identical source produce different bytes.
- **Behavior that depends on something off-disk** — a model pulled at startup, a remote config, a container image behind a wrapper script.

You are told when you forget. domarinn stores a digest of the program *on the cache entry* — never in the key — and a hit whose digest disagrees with what is on disk warns that it is replaying answers from a different build. Nothing is invalidated: whether a rebuild matters is the suite's call.

Anything else that steers the program should be an argument or an `env` entry rather than a salt: both are in the fingerprint, and [`${env:VAR}`](#environment-driven-config) lets you drive them from the ambient environment while keeping them keyed. A variable the child reads *without* the suite declaring it is invisible to the cache — see [caching.md](./caching.md#the-childs-environment-is-only-keyed-when-you-declare-it).

### Error and retry classification

An `exec` call is treated as **retriable** when the transport itself failed in a recoverable way — a **spawn** failure or a **timeout** — or when the child reports `{"error": {"retriable": true}}` in its response. A **non-zero exit**, **unparseable stdout**, or a child error with `retriable: false` is **fatal**. Retries follow the suite's `runner.retries` policy.

```yaml
providers:
  - id: my-service
    type: exec
    # `./provider.py` resolves to a file, so editing it busts the cache on its
    # own — no `cache_salt` needed here.
    command: ["./provider.py", "--model", "${env:MY_MODEL:-sonnet}"]
    env:
      MODEL_ENDPOINT: "${env:MODEL_ENDPOINT:-http://localhost:8080}"
    timeout_ms: 30000
```

---

## `anthropic`

A native client for the Anthropic **Messages API**.

| Field         | Type      | Default                     | Meaning |
|---------------|-----------|-----------------------------|---------|
| `model`       | string    | –                           | The model id. |
| `base_url`    | string    | `https://api.anthropic.com` | API base. |
| `api_key_env` | string    | `ANTHROPIC_API_KEY`         | Env var holding the API key (sent as `x-api-key`). |
| `params`      | object    | `{}`                        | Extra request-body params, passed through verbatim. |

Behavior:

- Calls `POST {base_url}/v1/messages` with header `anthropic-version: 2023-06-01`.
- **Params pass through verbatim** (`temperature`, `top_p`, `top_k`, `stop_sequences`, …). Nothing is forced; `model`, `messages`, and `system` are set by the client and are authoritative over any same-named param.
- **`max_tokens` defaults to 4096** — the API requires it, so it is filled in only when your `params` omit it.
- **System messages are extracted.** A chat prompt's `system`-role messages are pulled out and joined with blank lines into the top-level `system` field; the rest become `messages`. A plain text prompt becomes a single `user` message.
- Parses the response by concatenating `text` content blocks. Records token `usage` (cache reads and both cache-write TTLs included), `stop_reason`, and the `model` the API reports having served. `cost_usd` is computed from the built-in rate table; see [`pricing`](#pricing) to override it.
- A missing prompt is a fatal error (this provider requires a prompt).

```yaml
providers:
  - id: claude
    type: anthropic
    model: claude-sonnet-4-5
    params:
      temperature: 0.0
      max_tokens: 1024
```

---

## `openai`

An OpenAI-compatible **chat-completions** client. Works against the OpenAI API and any compatible gateway (vLLM, LiteLLM, Together, Ollama, …) via `base_url`.

| Field         | Type      | Default                      | Meaning |
|---------------|-----------|------------------------------|---------|
| `model`       | string    | –                            | The model id. |
| `base_url`    | string    | `https://api.openai.com/v1`  | API base — point this at any compatible gateway. |
| `api_key_env` | string    | `OPENAI_API_KEY`             | Env var holding the API key (sent as a bearer token). |
| `params`      | object    | `{}`                         | Extra request-body params, passed through verbatim. |

Behavior:

- Calls `POST {base_url}/chat/completions` with bearer auth.
- **Params pass through verbatim.** No temperature is forced; only `model` and `messages` are set by the client.
- A text prompt becomes a single `user` message; a chat prompt's messages pass through with their roles.
- Parses `choices[0].message.content` as the output, `finish_reason` as the stop reason, and `usage.prompt_tokens` / `usage.completion_tokens` as token usage. `cost_usd` is computed from domarinn's built-in rate table for known models; set `pricing:` to override the rates or price a model the table does not know.
- A missing prompt is a fatal error.

```yaml
providers:
  # OpenAI proper.
  - id: gpt
    type: openai
    model: gpt-4o-mini

  # A self-hosted OpenAI-compatible gateway.
  - id: local-llama
    type: openai
    model: llama-3.1-8b-instruct
    base_url: http://localhost:8000/v1
    api_key_env: LOCAL_GATEWAY_KEY
    params:
      temperature: 0.2
```

---

## Pricing

`anthropic` and `openai` providers cost each call from a built-in per-model rate table, so [`cost`](./assertions.md#budget-assertions-cost-latency-tokens) assertions and the run-level cost figure mean something without configuration.

A model the table does not know reports **no cost at all** rather than a guessed one — the `cost` assertion keeps honestly saying "not reported", and the run warns once naming the id. A made-up number that silently passes or fails a budget is worse than a loud no-op.

Override the rates, or price a model the table has never heard of, with `pricing` (USD per **million** tokens, merged field-wise over any built-in row):

```yaml
providers:
  - id: via-gateway
    type: anthropic
    model: our-fine-tune
    base_url: https://gateway.internal/v1
    pricing:
      input_per_mtok: 2.00
      output_per_mtok: 8.00
      cache_read_per_mtok: 0.20
      cache_write_per_mtok: 2.50
```

An `exec` provider that reports its own `cost_usd` always wins: it is the only party that knows whether it hit a proxy, a batch endpoint, or a different model entirely.

`pricing` never reaches a provider's fingerprint, so setting it does not invalidate a single cache entry. Cost is not request identity.

### Graders are priced too, and reported separately

`pricing` also works on a `grader.provider` block and on the `embeddings` provider, so the models doing the *scoring* are priced by the same table and the same override. What they cost is reported as `grader_cost_usd`, next to `cost_usd` rather than added to it:

- `cost_usd` is what the **systems under test** cost. That is the number a `cost:` assertion budgets, and the one a model-selection decision turns on. A judge's price must not move a budget gate on the model being judged.
- `grader_cost_usd` is what **measuring them** cost. On a suite scored by a larger model than it tests, this is the bigger of the two; merging them would hide that rather than report it.

Per-assertion, the same figure appears as `AssertResult.cost_usd`. An `exec` grader reports nothing — the child spends against whatever endpoint it chose and the protocol gives it no way to say so, and a zero there would claim custom grading is free.

## Credential preflight

Before the first call, domarinn checks that every credential the run will actually read resolves to a non-empty value, and fails with exit 2 naming the provider and the variable if not. "Actually read" is the operative part: a grader key is only required when a rubric assertion survived your filters.

It also rejects one known-wrong credential *shape* — an Anthropic OAuth access token (`sk-ant-oat…`), which the Messages API rejects as `x-api-key`. That is a hard failure only against `api.anthropic.com`; against any other `base_url` it is a warning, because a gateway may legitimately accept it.

Without this, a wrong **grader** key errors every case in the suite and exits 3, which reads as an infrastructure fault after burning the run's entire provider spend.

## `http`

A generic provider for black-box HTTP systems. The URL, headers, and body are **templated** (minijinja) against the test vars and the rendered `prompt`, and the response is projected to an output via an optional expression.

| Field         | Type                | Default | Meaning |
|---------------|---------------------|---------|---------|
| `url`         | string (templated)  | –       | Endpoint URL. |
| `method`      | string              | `POST`  | HTTP method. |
| `headers`     | `{string: string}` (values templated) | `{}` | Request headers. **In the cache key** (as a digest of the unrendered templates), so two providers differing only in `X-Model` do not share entries. |
| `body`        | JSON (templated)    | *(none)*| Request body, sent as JSON. |
| `output_expr` | string              | *(none)*| minijinja expression selecting the output from the response. |

Templating context for `url` / `headers` / `body`: every test var by name, plus `prompt` (the rendered prompt as a string).

The response is exposed to `output_expr` as:

```
response.status   # integer HTTP status
response.text     # raw body string
response.json     # parsed body (if it parsed), else null
response.headers  # response headers as an object
```

`output_expr` result handling: a string becomes a text output; any other value becomes a structured JSON output. **Without** `output_expr`, the raw response text is the output. This provider reports no token usage, cost, or stop reason.

**Caching.** `url`, `method`, `body`, `output_expr` and a digest of `headers` are all in the fingerprint, as written — unrendered. Test vars are already in the key separately, so the one input that can change the request without changing the key is the environment, and which syntax you use decides that:

- `${env:VAR}` resolves at **load time**, so the substituted value is in the fingerprint. Use it for anything that changes the answer — a model, an endpoint, a mode.
- `{{ env.VAR }}` renders **per request**, so only the template text is keyed. Use it for credentials, where keying the value would give every API key its own private cache.

A provider whose url, headers or body reference `{{ env.X }}` warns at startup naming the variable, because domarinn cannot tell a model selector from a token. See [caching.md](./caching.md#which-env-syntax).

```yaml
providers:
  - id: gateway
    type: http
    url: "https://api.example.com/v1/generate"
    method: POST
    headers:
      Authorization: "Bearer {{ api_token }}"     # from a test var
      Content-Type: "application/json"
    body:
      prompt: "{{ prompt }}"
      max_tokens: 256
    output_expr: "response.json.completion"
```

---

## `embeddings`

An OpenAI-compatible embeddings client. It is **not** a system under test — the runner filters it out of the graded matrix. Instead it powers the [`similar`](./assertions.md#similar) assertion: the **first** `type: embeddings` provider in the suite is handed to the grader.

| Field         | Type      | Default                      | Meaning |
|---------------|-----------|------------------------------|---------|
| `model`       | string    | –                            | The embeddings model id. |
| `base_url`    | string    | `https://api.openai.com/v1`  | API base. |
| `api_key_env` | string    | `OPENAI_API_KEY`             | Env var holding the API key (sent as a bearer token). |
| `params`      | object    | `{}`                         | Extra request-body params, passed through verbatim. |
| `pricing`     | object    | built-in rate                | Rate override. Only `input_per_mtok` is read — see below. |

Behavior:

- Calls `POST {base_url}/embeddings` with bearer auth and body `{ "model": …, "input": <text>, …params }`.
- Reads the vector from `data[0].embedding`. Cosine similarity between the output and the reference then drives `similar`.
- Listing an `embeddings` provider as a *direct* system under test is unsupported (the provider factory rejects it); it exists only to serve `similar`.
- Each `similar` assertion embeds **two** strings (the output and the reference), and both calls are priced and reported as that assertion's grading cost. Only `input_per_mtok` applies: an embedding call has no output tokens and the endpoint reports no cache counters, so the other `pricing` fields would price components that do not exist.

```yaml
providers:
  - id: sut
    type: openai
    model: gpt-4o-mini
  - id: embed                 # not tested directly; enables `similar`
    type: embeddings
    model: text-embedding-3-small

tests:
  - vars: { q: "capital of Japan" }
    assert:
      - type: similar
        value: "The capital of Japan is Tokyo."
        threshold: 0.85
```

---

## Retry, timeout, and error classification

The HTTP-backed providers (`anthropic`, `openai`, `http`, and the embeddings client) share one classification of failures, in `net.rs`. This keeps behavior consistent across every network provider.

| Condition                                   | Classification | Retriable? |
|---------------------------------------------|----------------|------------|
| HTTP `429` (Too Many Requests)              | retriable      | yes — honors `Retry-After` |
| HTTP `5xx` (server error)                   | retriable      | yes — honors `Retry-After` |
| Other `4xx` (bad request, auth, not found)  | fatal          | no |
| Timeout / connection / request build error  | retriable      | yes |
| Other transport errors                      | fatal          | no |
| Missing API key env var                     | fatal          | no |

Details:

- **`Retry-After` is honored.** On a `429`/`5xx` carrying a `Retry-After` header (delta-seconds form), that delay is used before the next attempt; otherwise the runner's exponential backoff applies.
- **Retries are opt-in.** The default is no retries (`runner.retries.max = 0`). Configure `runner.retries` to enable backoff (default initial 500 ms, max 8000 ms). A retriable error that exhausts attempts becomes a case `error` (exit code `3`); a fatal error becomes a case `error` immediately.
- **Default request timeout** for `anthropic`, `openai`, and `http` is **120 s**; the embeddings client uses **60 s**. The `exec` provider uses its own `timeout_ms` (default 60000 ms).
- **API keys never appear in cache keys.** A provider's cache identity (`fingerprint`) covers its type, model/command/url, params, and any `cache_salt`, but excludes secrets. For `exec` it also covers the program's identity and a digest of the declared `env` — a digest precisely because `env` is a credential channel, so the values never reach the stored entry.

The two failure classes map to `ProviderError::Retriable { retry_after }` and `ProviderError::Fatal` internally, which is what the runner's retry loop keys off.

---

## See also

- **[protocol.md](./protocol.md)** — the exec JSON protocol wire format for `exec` providers, asserts, and generators.
- **[caching.md](./caching.md)** — content-addressed caching, how an `exec` provider's program identity keys it, and when `cache_salt` is needed.
- **[assertions.md](./assertions.md)** — how provider outputs are graded, and the budget assertions that read `usage` / `cost_usd`.
- **[grading.md](./grading.md)** — using `anthropic` / `openai` providers as the LLM-rubric grader.
- **[configuration.md](./configuration.md)** — the full suite schema (`prompts`, `tests`, `defaults`, `runner`, `cache`).
</content>
