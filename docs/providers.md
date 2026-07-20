# Providers

A **provider** is a system under test: the thing measurellm sends inputs to and
grades the output of. Every suite lists at least one under `providers:`, and the
run matrix is `providers × prompts × tests × repeats`.

A provider is selected by its `type`. Five types exist:

| `type`       | What it is | System under test? |
|--------------|------------|--------------------|
| `exec`       | An external command speaking the exec JSON protocol (any language). | yes |
| `anthropic`  | Native Anthropic Messages API client. | yes |
| `openai`     | OpenAI-compatible chat-completions client (OpenAI + any compatible gateway). | yes |
| `http`       | An arbitrary templated HTTP endpoint. | yes |
| `embeddings` | An OpenAI-compatible `/embeddings` client. | no — it powers the [`similar`](./assertions.md#similar) assertion |

Every provider has an `id` (used in results and cache keys) and an optional
`label`. The remaining fields depend on `type`.

> Source of truth: `crates/measurellm-core/src/exec_provider.rs`,
> `anthropic.rs`, `openai.rs`, `http_provider.rs`, `embeddings.rs`, the shared
> networking in `net.rs`, and the `ProviderKind` schema in `config.rs`.

---

## `exec` — the flagship

An `exec` provider shells out to a command that speaks the **exec JSON
protocol**. If your program can read JSON from stdin and write JSON to stdout,
it is a provider — no Rust, no SDK.

| Field        | Type                | Default    | Meaning |
|--------------|---------------------|------------|---------|
| `command`    | `[string]`          | –          | The command and its argv. |
| `env`        | `{string: string}`  | `{}`       | Extra environment variables for the child. |
| `timeout_ms` | integer             | `60000`    | Per-call timeout in milliseconds. |
| `cache_salt` | string              | *(none)*   | Cache-busting token. **Required to cache.** |

### Wire behavior

For each call the provider writes one `provider` request to the child's stdin
and closes it, then reads one JSON response from stdout:

- **Request** (measurellm → child stdin):
  `{ "measurellm": {"protocol": 1, "kind": "provider"}, "prompt"?, "vars", "params", "test": {"id", "tags"} }`.
  `prompt` is **null / omitted** when the suite has no prompts (the "self-input"
  case) — the provider works from `vars` alone. A text prompt is sent as
  `{ "text": "…" }`; a chat prompt as `{ "messages": [...] }`.
- **Response** (child stdout → measurellm):
  `{ "output" (required), "usage"?, "cost_usd"?, "error"?, "metadata"? }`.
  A string `output` becomes text; any other JSON becomes a structured output.
  `usage` fills token counts, `cost_usd` feeds the [`cost`](./assertions.md#budget-assertions-cost-latency-tokens)
  assertion, and `metadata` is retained as the raw payload.

The child **always** receives `MEASURELLM_PROTOCOL=1` in its environment, plus
your `env`. The full wire contract, exit-code rules, and minimal Bash/Python
examples live in **[protocol.md](./protocol.md)**.

### Caching requires `cache_salt`

`exec` providers are **not cacheable** unless `cache_salt` is set. Because
measurellm cannot see inside your binary, it will not risk serving stale output
from a rebuilt program: with no salt every call runs fresh. Set `cache_salt` to
a value that changes whenever your program's behavior changes — a git SHA, a
binary hash, a version string. See [caching.md](./caching.md#cache_salt).

### Error and retry classification

An `exec` call is treated as **retriable** when the transport itself failed in a
recoverable way — a **spawn** failure or a **timeout** — or when the child
reports `{"error": {"retriable": true}}` in its response. A **non-zero exit**,
**unparseable stdout**, or a child error with `retriable: false` is **fatal**.
Retries follow the suite's `runner.retries` policy.

```yaml
providers:
  - id: my-service
    type: exec
    command: ["./provider.py"]
    env:
      MODEL_ENDPOINT: "http://localhost:8080"
    timeout_ms: 30000
    cache_salt: "v3"       # bump when ./provider.py changes
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
- **Params pass through verbatim** (`temperature`, `top_p`, `top_k`, `stop_sequences`, …).
  Nothing is forced; `model`, `messages`, and `system` are set by the client and
  are authoritative over any same-named param.
- **`max_tokens` defaults to 4096** — the API requires it, so it is filled in
  only when your `params` omit it.
- **System messages are extracted.** A chat prompt's `system`-role messages are
  pulled out and joined with blank lines into the top-level `system` field; the
  rest become `messages`. A plain text prompt becomes a single `user` message.
- Parses the response by concatenating `text` content blocks. Records token
  `usage` (including `cache_read_input_tokens`) and `stop_reason`. It does not
  compute `cost_usd`.
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

An OpenAI-compatible **chat-completions** client. Works against the OpenAI API
and any compatible gateway (vLLM, LiteLLM, Together, Ollama, …) via `base_url`.

| Field         | Type      | Default                      | Meaning |
|---------------|-----------|------------------------------|---------|
| `model`       | string    | –                            | The model id. |
| `base_url`    | string    | `https://api.openai.com/v1`  | API base — point this at any compatible gateway. |
| `api_key_env` | string    | `OPENAI_API_KEY`             | Env var holding the API key (sent as a bearer token). |
| `params`      | object    | `{}`                         | Extra request-body params, passed through verbatim. |

Behavior:

- Calls `POST {base_url}/chat/completions` with bearer auth.
- **Params pass through verbatim.** No temperature is forced; only `model` and
  `messages` are set by the client.
- A text prompt becomes a single `user` message; a chat prompt's messages pass
  through with their roles.
- Parses `choices[0].message.content` as the output, `finish_reason` as the
  stop reason, and `usage.prompt_tokens` / `usage.completion_tokens` as token
  usage. It does not compute `cost_usd`.
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

## `http`

A generic provider for black-box HTTP systems. The URL, headers, and body are
**templated** (minijinja) against the test vars and the rendered `prompt`, and
the response is projected to an output via an optional expression.

| Field         | Type                | Default | Meaning |
|---------------|---------------------|---------|---------|
| `url`         | string (templated)  | –       | Endpoint URL. |
| `method`      | string              | `POST`  | HTTP method. |
| `headers`     | `{string: string}` (values templated) | `{}` | Request headers. |
| `body`        | JSON (templated)    | *(none)*| Request body, sent as JSON. |
| `output_expr` | string              | *(none)*| minijinja expression selecting the output from the response. |

Templating context for `url` / `headers` / `body`: every test var by name, plus
`prompt` (the rendered prompt as a string).

The response is exposed to `output_expr` as:

```
response.status   # integer HTTP status
response.text     # raw body string
response.json     # parsed body (if it parsed), else null
response.headers  # response headers as an object
```

`output_expr` result handling: a string becomes a text output; any other value
becomes a structured JSON output. **Without** `output_expr`, the raw response
text is the output. This provider reports no token usage, cost, or stop reason.

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

An OpenAI-compatible embeddings client. It is **not** a system under test — the
runner filters it out of the graded matrix. Instead it powers the
[`similar`](./assertions.md#similar) assertion: the **first** `type: embeddings`
provider in the suite is handed to the grader.

| Field         | Type      | Default                      | Meaning |
|---------------|-----------|------------------------------|---------|
| `model`       | string    | –                            | The embeddings model id. |
| `base_url`    | string    | `https://api.openai.com/v1`  | API base. |
| `api_key_env` | string    | `OPENAI_API_KEY`             | Env var holding the API key (sent as a bearer token). |
| `params`      | object    | `{}`                         | Extra request-body params, passed through verbatim. |

Behavior:

- Calls `POST {base_url}/embeddings` with bearer auth and body
  `{ "model": …, "input": <text>, …params }`.
- Reads the vector from `data[0].embedding`. Cosine similarity between the
  output and the reference then drives `similar`.
- Listing an `embeddings` provider as a *direct* system under test is
  unsupported (the provider factory rejects it); it exists only to serve
  `similar`.

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

The HTTP-backed providers (`anthropic`, `openai`, `http`, and the embeddings
client) share one classification of failures, in `net.rs`. This keeps behavior
consistent across every network provider.

| Condition                                   | Classification | Retriable? |
|---------------------------------------------|----------------|------------|
| HTTP `429` (Too Many Requests)              | retriable      | yes — honors `Retry-After` |
| HTTP `5xx` (server error)                   | retriable      | yes — honors `Retry-After` |
| Other `4xx` (bad request, auth, not found)  | fatal          | no |
| Timeout / connection / request build error  | retriable      | yes |
| Other transport errors                      | fatal          | no |
| Missing API key env var                     | fatal          | no |

Details:

- **`Retry-After` is honored.** On a `429`/`5xx` carrying a `Retry-After`
  header (delta-seconds form), that delay is used before the next attempt;
  otherwise the runner's exponential backoff applies.
- **Retries are opt-in.** The default is no retries (`runner.retries.max = 0`).
  Configure `runner.retries` to enable backoff (default initial 500 ms, max
  8000 ms). A retriable error that exhausts attempts becomes a case `error`
  (exit code `3`); a fatal error becomes a case `error` immediately.
- **Default request timeout** for `anthropic`, `openai`, and `http` is **120 s**;
  the embeddings client uses **60 s**. The `exec` provider uses its own
  `timeout_ms` (default 60000 ms).
- **API keys never appear in cache keys.** A provider's cache identity
  (`fingerprint`) covers its type, model/command/url, params, and any
  `cache_salt`, but excludes secrets.

The two failure classes map to `ProviderError::Retriable { retry_after }` and
`ProviderError::Fatal` internally, which is what the runner's retry loop keys
off.

---

## See also

- **[protocol.md](./protocol.md)** — the exec JSON protocol wire format for
  `exec` providers, asserts, and generators.
- **[caching.md](./caching.md)** — content-addressed caching and the
  `cache_salt` requirement for `exec` providers.
- **[assertions.md](./assertions.md)** — how provider outputs are graded, and
  the budget assertions that read `usage` / `cost_usd`.
- **[grading.md](./grading.md)** — using `anthropic` / `openai` providers as
  the LLM-rubric grader.
- **[configuration.md](./configuration.md)** — the full suite schema
  (`prompts`, `tests`, `defaults`, `runner`, `cache`).
</content>
