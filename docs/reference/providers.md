<!-- Canonicality: domarinn-yaml.md documents the suite file's shape — every top-level
key, once. Any key whose meaning varies by provider type is documented once in
providers.md. If you are writing the same sentence in both places, it belongs in
providers.md. -->

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

Any string in a provider's configuration — including elements of an `exec` provider's `command` argv and `env` map — may contain a `${env:VAR}` placeholder, resolved once at load time — handy for a per-developer endpoint or a per-environment gateway that shouldn't be committed. The full rules (`:-default`, the `$${...}` escape, and exactly which parts of the suite this covers) are documented once, in [domarinn.yaml → Environment interpolation](./domarinn-yaml.md#environment-interpolation-envvar).

---

## `exec`

The flagship provider, and the escape hatch for testing anything you can run as a process: it shells out to a command that speaks the **exec JSON protocol**. If your program can read JSON from stdin and write JSON to stdout, it is a provider — no Rust, no SDK.

| Field        | Type                | Default    | Meaning |
|--------------|---------------------|------------|---------|
| `command`    | `[string]`          | –          | The command and its argv. Elements may contain [`${env:VAR}`](#environment-driven-config) placeholders. |
| `env`        | `{string: string}`  | `{}`       | Extra environment variables for the child. Values may contain `${env:VAR}` placeholders too. |
| `timeout_ms` | integer             | `60000`    | Per-call timeout in milliseconds. |
| `cache_salt` | string              | *(none)*   | **Provider-level** version pin for the program — set it when a rebuild should discard cached answers. See below. Distinct from a test's own [`cache_salt`](./domarinn-yaml.md#inline-and-loaded-test-fields), which keys that test's cases instead; see [caching.md](../concepts/caching.md#the-rule). |

### Wire behavior

For each call the provider writes one `provider` request to the child's stdin and closes it, then reads one JSON response from stdout:

- **Request** (domarinn → child stdin): `{ "domarinn": {"protocol": 1, "kind": "provider"}, "prompt"?, "vars", "params", "test": {"id", "tags"} }`. `prompt` is **null / omitted** when the suite has no prompts (the "self-input" case) — the provider works from `vars` alone. A text prompt is sent as `{ "text": "…" }`; a chat prompt as `{ "messages": [...] }`.
- **Response** (child stdout → domarinn): `output` is the only required field; see [the protocol reference](./protocol.md#response) for the full set. A string `output` becomes text; any other JSON becomes a structured output. `usage` fills token counts, `cost_usd` feeds the [`cost`](./assertions.md#budget-assertions-cost-latency-tokens) assertion, and `metadata` is retained as the raw payload.
- Worth reporting even though all of it is optional: `empty_reason` (so a refusal is diagnosed instead of scoring zero against every assertion), `error.class` (so a rejected credential is distinguishable from a crash), `error.details` (structured diagnostics that survive to the stored case), and `model` (so an alias that silently repointed is visible).

The child **always** receives `DOMARINN_PROTOCOL=1` in its environment, plus your `env`. The full wire contract, exit-code rules, and minimal Bash/Python examples live in **[protocol.md](./protocol.md)**.

### Caching, and when you need `cache_salt`

`exec` providers are **cached by default**, under the same one rule as every other call: the key is a hash of the request — the command, its args, the protocol document on the child's stdin, and a digest of the declared `env`. It says nothing about the program's *bytes*, so an entry written on one machine is reusable on every other: a fresh clone, a different checkout path, a rebuilt binary and a different working directory all key identically.

The price is that domarinn cannot tell one build of your program from the next, so **set `cache_salt` when a rebuild should discard the old answers** — a commit SHA, a release tag, or `"$digest: src/**/*.rs"`. Forget, and a hit whose stored program digest disagrees with what is on disk *warns*; nothing is invalidated, because whether a rebuild matters is the suite's call.

Anything else that steers the program belongs in argv or `env` rather than in a salt, where the key can see it. [`${env:VAR}`](#environment-driven-config) drives those from the ambient environment while keeping them keyed; a variable the child reads *without* the suite declaring it is invisible to the cache.

Full details — the two salt levels, `$digest:`, and what the child's environment does and does not key — are in [caching.md](../concepts/caching.md#exec-providers-and-the-provider-salt).

### Error and retry classification

An `exec` call is treated as **retriable** when the transport itself failed in a recoverable way — a **spawn** failure or a **timeout** — or when the child reports `{"error": {"retriable": true}}` in its response. A **non-zero exit**, **unparseable stdout**, or a child error with `retriable: false` is **fatal**. Retries follow the suite's `runner.retries` policy.

```yaml
--8<-- "examples/13-exec-provider/domarinn.yaml:provider"
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
--8<-- "examples/27-anthropic-provider/domarinn.yaml:provider"
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
--8<-- "examples/26-openai-provider/domarinn.yaml:provider"
```

The same shape works unchanged against a self-hosted gateway — only `base_url` and `api_key_env` differ:

```yaml
providers:
  # A self-hosted OpenAI-compatible gateway (vLLM, LiteLLM, Ollama, ...).
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
--8<-- "examples/27-anthropic-provider/domarinn.yaml:pricing"
```

That works just as well for a model the built-in table has never heard of — a negotiated rate, a preview model, or a fine-tune behind a gateway — as it does for overriding a known one.

An `exec` provider that reports its own `cost_usd` always wins: it is the only party that knows whether it hit a proxy, a batch endpoint, or a different model entirely.

`pricing` is not part of any request, so setting it does not invalidate a single cache entry — cost is re-derived on every hit at the current rate. Cost is not request identity.

### Graders are priced too, and reported separately

`pricing` also works on a `grader.provider` block and on the `embeddings` provider, so the models doing the *scoring* are priced by the same table and the same override. What they cost is reported as `grader_cost_usd`, next to `cost_usd` rather than added to it:

- `cost_usd` is what the **systems under test** cost. That is the number a `cost:` assertion budgets, and the one a model-selection decision turns on. A grader's price must not move a budget gate on the model being judged.
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
| `headers`     | `{string: string}` (values templated) | `{}` | Request headers. **In the cache key**, as a digest rather than the values, so two providers differing only in `X-Model` do not share entries while an `Authorization` holding two teammates' tokens still does. |
| `body`        | JSON (templated)    | *(none)*| Request body, sent as JSON. |
| `output_expr` | string              | *(none)*| minijinja expression selecting the output from the response. **In the cache key**: an entry stores the projected output, so changing the expression re-asks. |

Templating context for `url` / `headers` / `body`: every test var by name, plus `prompt` (the rendered prompt as a string).

The response is exposed to `output_expr` as:

```
response.status   # integer HTTP status
response.text     # raw body string
response.json     # parsed body (if it parsed), else null
response.headers  # response headers as an object
```

`output_expr` result handling: a string becomes a text output; any other value becomes a structured JSON output. **Without** `output_expr`, the raw response text is the output. This provider reports no token usage, cost, or stop reason.

**Caching.** The key is the request this provider would send: the **rendered** `method`, `url` and `body`, plus a digest of the rendered `headers` — and the configured `output_expr`, which never goes on the wire but decides what a stored answer *means*, so editing it busts that provider's entries. Test vars are in the key by way of the templates they render into.

One input can still change what happens without changing the key, and it is worth knowing:

- **The environment**, depending on which syntax you use. `${env:VAR}` resolves at **load time**, so the substituted value is keyed — use it for anything that changes the answer. `{{ env.VAR }}` renders **per request** and is keyed as a literal `${env:NAME}` placeholder — use it for credentials, where keying the value would give every API key its own private cache. A provider whose url, headers or body reference `{{ env.X }}` warns at startup naming the variable, because domarinn cannot tell a model selector from a token. See [caching.md](../concepts/caching.md#which-env-syntax).

```yaml
--8<-- "examples/28-http-provider/domarinn.yaml:provider"
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
--8<-- "examples/30-similar-embeddings/domarinn.yaml:provider"
```

See [`similar`](./assertions.md#similar) for the assertion this provider exists to serve.

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
- **API keys never appear in cache keys.** The key hashes the *redacted* request: credentials live in headers, which the keyed envelope excludes structurally, and an `exec` provider's declared `env` enters as a digest — precisely because `env` is a credential channel, so the values never reach the stored entry. What the key does cover for `exec` is the command and its args plus the document sent to the child.

The two failure classes map to `ProviderError::Retriable { retry_after }` and `ProviderError::Fatal` internally, which is what the runner's retry loop keys off.

---

## See also

- **[protocol.md](./protocol.md)** — the exec JSON protocol wire format for `exec` providers, asserts, and generators.
- **[caching.md](../concepts/caching.md)** — the one key rule, every cache knob in one table, and when `cache_salt` is needed.
- **[assertions.md](./assertions.md)** — how provider outputs are graded, and the budget assertions that read `usage` / `cost_usd`.
- **[grading.md](../concepts/grading.md)** — using `anthropic` / `openai` providers as the LLM-rubric grader.
- **[domarinn.yaml](./domarinn-yaml.md)** — the full suite schema (`prompts`, `tests`, `defaults`, `runner`, `cache`).
