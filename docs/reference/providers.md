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
| `embeddings` | An OpenAI-compatible `/embeddings` client. | no — it powers the [`similar`](assertions.md#similar) assertion |

Every provider has an `id` (used in results and cache keys) and an optional `label`. The remaining fields depend on `type`.

> Source of truth: `crates/domarinn-core/src/exec_provider.rs`,
> `anthropic.rs`, `openai.rs`, `http_provider.rs`, `embeddings.rs`, the shared
> networking in `net.rs`, and the `ProviderKind` schema in `config.rs`.

### Environment-driven config

Any string in a provider's configuration — including elements of an `exec` provider's `command` argv and `env` map — may contain a `${env:VAR}` placeholder, resolved once at load time — handy for a per-developer endpoint or a per-environment gateway that shouldn't be committed. The full rules (`:-default`, the `$${...}` escape, and exactly which parts of the suite this covers) are documented once, in [domarinn.yaml → Environment interpolation](domarinn-yaml.md#environment-interpolation-envvar).

---

## `exec`

The flagship provider, and the escape hatch for testing anything you can run as a process: it shells out to a command that speaks the **exec JSON protocol**. If your program can read JSON from stdin and write JSON to stdout, it is a provider — no Rust, no SDK.

| Field        | Type                | Default    | Meaning |
|--------------|---------------------|------------|---------|
| `command`    | `[string]`          | –          | The command and its argv. Elements may contain [`${env:VAR}`](#environment-driven-config) placeholders. |
| `env`        | `{string: string}`  | `{}`       | Extra environment variables for the child. Values may contain `${env:VAR}` placeholders too. |
| `timeout_ms` | integer             | `60000`    | Per-call timeout in milliseconds. |
| `cache_salt` | string              | *(none)*   | **Provider-level** version pin for the program — set it when a rebuild should discard cached answers. See below. Distinct from a test's own [`cache_salt`](domarinn-yaml.md#inline-and-loaded-test-fields), which keys that test's cases instead; see [caching.md](../concepts/caching.md#the-rule). |

### Wire behavior

For each call the provider writes one `provider` request to the child's stdin and closes it, then reads one JSON response from stdout:

- **Request** (domarinn → child stdin): `{ "domarinn": {"protocol": 1, "kind": "provider"}, "prompt"?, "vars", "params", "test": {"id", "tags"} }`. `prompt` is **null / omitted** when the suite has no prompts (the "self-input" case) — the provider works from `vars` alone. A text prompt is sent as `{ "text": "…" }`; a chat prompt as `{ "messages": [...] }`.
- **Response** (child stdout → domarinn): `output` is the only required field; see [the protocol reference](protocol.md#response) for the full set. A string `output` becomes text; any other JSON becomes a structured output. `usage` fills token counts, `cost_usd` feeds the [`cost`](assertions.md#budget-assertions-cost-latency-tokens) assertion, and `metadata` is retained as the raw payload.
- Worth reporting even though all of it is optional: `empty_reason` (so a refusal is diagnosed instead of scoring zero against every assertion), `error.class` (so a rejected credential is distinguishable from a crash), `error.details` (structured diagnostics that survive to the stored case), and `model` (so an alias that silently repointed is visible).

The child **always** receives `DOMARINN_PROTOCOL=1` in its environment, plus your `env`. The full wire contract, exit-code rules, and worked Bash/Python examples live in **[protocol.md](protocol.md)**.

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
| `request`     | object    | –                           | Transport overrides — auth scheme, headers, path, query, body overlay. See [Customizing the request](#customizing-the-request). |
| `cache_salt`  | string    | –                           | Cache pin. Change it to throw away every answer this provider has cached. |

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

`anthropic` and `openai` providers cost each call from a built-in per-model rate table, so [`cost`](assertions.md#budget-assertions-cost-latency-tokens) assertions and the run-level cost figure mean something without configuration.

A model the table does not know reports **no cost at all** rather than a guessed one — the `cost` assertion keeps honestly saying "not reported", and the run warns once naming the id. A made-up number that silently passes or fails a budget is worse than a loud no-op.

Ids rarely arrive in their plainest form, so three shapes resolve to the same row before that gives up. A dated snapshot has its date stripped (`claude-opus-5-20260315`, `gpt-4o-2024-08-06`); a Bedrock or Vertex decoration is peeled off (`us.anthropic.claude-opus-5`, `claude-haiku-4-5@20251001`); and anything left over falls back to the **longest matching model stem**, which is what prices suffixed aliases and point releases nobody enumerated:

```
claude-sonnet-5-latest  →  claude-sonnet-5     ($3.00 / $15.00 per MTok)
```

That fallback is deliberately narrow. A stem only earns a fallback entry when no differently-priced sibling shares its prefix, which is why `gpt-5`, `o3`, `gpt-4o` and `gpt-4o-mini` have exact rows but no fallback: `gpt-5-pro`, `o3-mini`, and the `gpt-4o-*` audio-pipeline models (`gpt-4o-mini-tts`, `gpt-4o-mini-transcribe`) would inherit a rate that is off by several times. Models whose published price varies with context length — OpenAI's current flagships — are left out for the same reason, since a single rate cannot express two. Those ids stay unpriced and warn, and a `pricing:` block is how you price one anyway.

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

The complaint is about how the credential is **presented**, not about the credential, so it does not fire for a provider that sets [`request: {auth: bearer}`](#customizing-the-request) — that is the supported fix, not a workaround. A provider with `auth: none` reads no credential at all, so nothing is checked for it.

Without this, a wrong **grader** key errors every case in the suite and exits 3, which reads as an infrastructure fault after burning the run's entire provider spend.

## Customizing the request

`anthropic`, `openai`, and `embeddings` know how to *shape* a request for their vendor. `request:` controls the *envelope* that carries it — everything about who you say you are, where you send it, and what rides alongside the body.

| Field     | Type                | Meaning |
|-----------|---------------------|---------|
| `auth`    | `api_key` \| `bearer` \| `none` | How the credential from `api_key_env` is presented. Defaults to the vendor's own scheme. `none` sends nothing and requires no credential. |
| `path`    | string              | Replaces the endpoint path appended to `base_url` (`/v1/messages`, `/chat/completions`, `/embeddings`). Must start with `/`. |
| `query`   | object              | Query parameters. Sorted by name before sending. |
| `headers` | object              | Headers added to the request, overriding the vendor's own by name (case-insensitively). |
| `body`    | object              | Fields merged into the body **last**, after the provider built it. |

Every value is a minijinja template rendered against `env`. They are rendered **once**, when the provider is built — not per case. A header that must vary per case is what [`type: http`](#http) is for.

### Presenting an OAuth token

The motivating case. An Anthropic OAuth access token is rejected as `x-api-key` and accepted as a bearer token:

```yaml
--8<-- "examples/43-custom-request/domarinn.yaml:provider"
```

domarinn performs no OAuth flow of its own: it does not fetch, refresh, or cache a token. Keeping one valid is the caller's job — a wrapper script, a CI step, whatever already mints it. `auth: bearer` only decides how the token you supply is presented.

### `body:` reaches what `params:` cannot

`params:` merges **first**, and `model`, `messages`, and `system` are then written over it. Those three are exactly the fields a gateway most often needs changed — a routed model name, an injected system prompt — so `params:` structurally cannot reach them. `request.body` merges **last**, and can.

It is a deep merge: an object value merges key by key, anything else replaces. Overwriting `messages` is possible and is almost always a mistake.

### `DOMARINN_PROVIDER_HEADERS`

Headers merged into every HTTP-speaking provider, as a JSON object, without editing a suite:

```sh
export DOMARINN_PROVIDER_HEADERS='{"x-corp-egress":"prod-gw-7","x-cost-center":"eng-platform"}'
```

For an environment that must add a header to traffic it does not own the suites for. A suite's own `request.headers` wins by name — the variable supplies a default, and a suite that named the header meant it. Values are templates, rendered exactly like a suite's, so a credential written `{{ env.X }}` is redacted from the cache the same way.

Malformed JSON, or an object whose values are not strings, is a hard error rather than a silent skip: it was exported because a gateway requires it, and a dropped egress header fails at the far end with no local evidence.

These headers **are** part of the cache key. Exporting the variable in CI and not locally therefore splits the cache in two, which is the cost of treating them as request content rather than as transport.

## `http`

A generic provider for black-box HTTP systems. The URL, headers, and body are **templated** (minijinja) against the test vars and the rendered `prompt`, and the response is projected to an output via an optional expression.

| Field         | Type                | Default | Meaning |
|---------------|---------------------|---------|---------|
| `url`         | string (templated)  | –       | Endpoint URL. |
| `method`      | string              | `POST`  | HTTP method. |
| `headers`     | `{string: string}` (values templated) | `{}` | Request headers. **In the cache key**, as a digest rather than the values, so two providers differing only in `X-Model` do not share entries while an `Authorization` holding two teammates' tokens still does. |
| `body`        | JSON (templated)    | *(none)*| Request body, sent as JSON. |
| `output_expr` | string              | *(none)*| minijinja expression selecting the output from the response. **In the cache key**: an entry stores the projected output, so changing the expression re-asks. |

Templating context for `url` / `headers` / `body`: every test var by name, plus two views of the rendered prompt:

- `prompt` — the prompt as one string. A multi-turn prompt (a `messages:` prompt, or a case with [history](domarinn-yaml.md#per-case-history)) is flattened to `role: content` lines joined by newlines. **Prose only**: a turn's tool calls and its `thinking` blocks are deliberately absent, because inventing a textual rendering for a tool call is exactly what invites a tool-eager model to imitate that syntax as text instead of emitting a real call.
- `messages` — the same turns **structurally**, a list of `{role, content}` objects — plus `tool_calls` / `tool_call_id` on a [tool-using transcript](domarinn-yaml.md#tool-using-transcripts), which is the view that carries them: `{{ messages | tojson }}` embeds the array JSON-encoded, and `{% for m in messages %}` iterates it — the way a body template forwards a real conversation to an OpenAI-shaped API. A `template:` prompt appears as the single user turn it becomes on the wire.

A test var named `messages` takes precedence: the structural view is only added when no var of that name exists, so a suite that already forwarded a hand-rolled conversation under that name keeps rendering — and cache-keying — exactly as before. Existing templates that never reference `messages` render byte-identically either way.

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

An OpenAI-compatible embeddings client. It is **not** a system under test — the runner filters it out of the graded matrix. Instead it powers the [`similar`](assertions.md#similar) assertion: the **first** `type: embeddings` provider in the suite is handed to the grader.

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

See [`similar`](assertions.md#similar) for the assertion this provider exists to serve.

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

## Falling back to another provider

Retries answer a provider that failed to reply. `fallback:` answers a provider that replied and said no — or one that is simply not there this morning. A refusal is not a verdict about the prompt, and a gateway being down is not a regression in the system under test, but without somewhere to go both become exactly that: a failed case and a red gate.

```yaml
--8<-- "examples/44-provider-fallback/domarinn.yaml:provider"
```

The list names other providers in the same suite, tried in order. The cell still belongs to `primary`: the request is handed on verbatim, and whoever answers, the case is recorded against the provider you configured.

### When a link hands off

Two kinds of outcome, both keyed on open string types so a value this build has never heard of round-trips rather than being swallowed.

**An empty answer whose reason is in the trigger set.** `runner.fallback_on_empty_reason` defaults to `["refusal", "content_filter"]` — the two that mean *this provider will not answer this*, as opposed to *this provider answered badly*, which is a result and belongs to the grader. Set it to `[]` to hand off only on hard failures, or widen it to any reason [the classifier](../concepts/grading.md#empty-outputs-and-grading) can produce.

**A call that failed outright**, in one of six classes:

| Class | |
|---|---|
| `provider_auth` | a rejected or missing credential |
| `provider_unavailable` | `5xx`, connection refused, DNS |
| `provider_timeout` | no reply inside the timeout |
| `provider_protocol` | a reply that could not be understood |
| `provider_rate_limit` | `429` after retries are exhausted |
| `exec_failed` | an `exec` child that did not run |

This is an explicit list rather than "any infrastructure error", for two reasons. `cache_miss` and `cache_unavailable` classify as infrastructure and must **never** hand off — that would turn an offline run into a live call. And `provider_request` is deliberately absent: a `400` for a malformed body is the suite's bug, and the next provider will reject it identically, so spending a second call to learn that is noise rather than resilience.

### Four things a reader can rely on

1. **Never under `--cache-only`.** Offline there is no live answer to go and get, so a handoff could only replace a usable — if refused — replay with a cache miss. This is a mode check, not a property of the trigger list, so no future trigger can weaken it.
2. **Never for a cell carrying a `latency` assert.** Such a cell is forced to cache-disabled rather than cache-only, so rule 1 does not cover it, and `latency_ms` is the *answering* link's time in flight. Since `provider_timeout` is a trigger, primary-times-out then fallback-answers-fast would make `{latency, max: 2000}` **pass**. "Never worse" needs a matching "never falsely better".
3. **The primary is reported when nothing improved on it.** If every link is also a trigger, the case settles on the primary's own outcome rather than the last link's — so a configured `fallback:` can never make a case different from one with no fallback at all. Without this, `provider_digest` would churn for zero gain and the run document would diverge from a no-fallback run, which breaks the server's re-upload idempotency.
4. **Chains are not followed.** A fallback's own `fallback:` is ignored when it is reached as one, which makes a cycle unconstructible rather than something to detect mid-run. `domarinn validate` **warns** when a target declares one, so the rule is discoverable while you are writing the suite. It also errors on an unknown id, a self-reference, and any `fallback:` naming — or living on — an `embeddings` provider, which is a grader helper and never a system under test.

A fallback that fails to build — no credential in the environment, say — is warned about and dropped, not fatal. Fallback providers are excluded from the credential preflight for the same reason: failing a whole run over a provider nothing selected would make `fallback:` a liability to configure rather than cheap insurance.

### What the results say

`cell.provider_id` stays the **configured** provider, whichever link answered. That is what keeps `case_key` stable, so an `--against` baseline still joins the same row and a suite does not silently re-partition its history the first time a gateway hiccups.

What actually happened is recorded rather than hidden:

| Field | |
|---|---|
| `CaseResult.answered_by_provider_id` | who replied, set only when it was not the configured provider |
| `CaseResult.fallback_attempts` | each link tried and passed over, in order, with its `empty_reason` or `error_class` |
| `CaseResult.provider_digest` | the answering link's fingerprint, not the configured provider's |
| `RunSummary.fallback_cases` | how many cases needed one |

All four are omitted at their defaults, so a run that never fell back serializes byte-identically to one written before the feature existed. A **server older than this feature** accepts such a run and then drops the fields permanently, because it re-serializes its own typed struct on ingest.

Because the digest moves, the server's [compare view](web-ui.md#compare--mcnemar) classifies such a pair as `ProviderChanged` — which is what it is, and it does so whether the case passed or failed. The CLI's `--against` is coarser: it reports a delta (newly failing, newly passing, unchanged) and does not carry that axis, so **a case answered by a fallback on both sides reads as `Unchanged`** even though two different models produced it. When a comparison matters, read `fallback_cases` on both runs before reading the delta.

**A run where every graded case fell back exits `2`.** The suite ran, but not against the system it names, and a green gate would say otherwise. A partial fallback stays green — that is the feature working. [`--no-fallback`](cli.md#domarinn-run-path-flags) is the recommended posture for a gate that would rather fail on its primary directly.

### Filters, and which one applies

A test's `only_providers` / `skip_providers` **is** honoured for fallback candidates: a test that excludes a provider must not reach it through another provider's back door.

`--provider` is **not**. It chooses which *cells* run, and a cell that runs is entitled to its whole chain — a run that silently lost the resilience you configured would look exactly like fallback not working. So `domarinn run --provider primary` runs only `primary`'s cells, and each of them may still reach `backup`.

### Cost attribution is per-case correct and per-provider wrong

Worth stating plainly, because the failure mode is a number that looks right.

A case's `cost_usd` is **correct**: it is re-priced at the answering provider's rate. But the results server promotes `cell.provider_id` — the configured provider — into its `cases` table, so **a per-provider cost rollup bills the primary for the fallback's tokens.** A dashboard slicing spend by provider will under-report the fallback and over-report the primary by exactly the cases that handed off; `fallback_cases` is how you tell whether that is a rounding error or the whole picture.

Two smaller gaps in the same direction, since only the answering link is summarized: the primary's own spend on a handoff is not counted at all, a retry on the primary does not reach `retried_cases`, and a hit-then-handoff counts as a miss. Per-case dollars stay right; it is attribution that is lost.

### Prose refusals: `runner.refusal_patterns`

A refusal is only classified as one when the vendor sets that finish reason. A model that answers *"I can't help with that request."* in ordinary prose is, as far as every mechanism above is concerned, a normal answer — so it is graded, and it is cached.

`runner.refusal_patterns` is the opt-in for that case: a list of regexes, empty by default, and an output matching any of them is treated as an effective `refusal`.

```yaml
runner:
  refusal_patterns:
    - "(?i)^i (can't|cannot|won't) help with"
```

Three things to know before using it:

- **A false positive silently swaps in a different model.** A pattern loose enough to match a legitimate answer hands that case to the fallback and reports a `ProviderChanged`, not an error. Anchor the pattern; test it against outputs you expect to keep.
- **A `not-contains` assertion is the deterministic alternative.** If what you want is *this case must fail when the model refuses*, assert it. That is a verdict about the response, computed the same way every run, with no dependence on which providers happen to be configured.
- **The pattern is re-applied on every read**, never written onto a cache entry, so editing one reclassifies what is already stored rather than requiring a purge. An invalid regex is a `validate` error.

Patterns compile once per run, and matching a JSON output uses its rendered form — a refusal that arrived as `{"error": "I can't help with that"}` is still a refusal.

### `${env:VAR}` works here, and is not a secret channel

Interpolation runs over the raw document before anything is parsed, so `fallback: ["${env:FALLBACK_PROVIDER}"]` works like any other config string, with the usual `:-default` rules. `fallback:` itself sits on the provider's outer struct and never reaches a fingerprint or a canonical request, so no value of it can move a single cache key — turning it on for an existing suite invalidates nothing.

/// danger | Do not read that as "so environment values are safe to put anywhere"

Everywhere `${env:…}` reaches something that *is* part of the request — a model, an endpoint path, an `exec` argv — it is resolved at load time and the substituted value is **in the cache key**. That is correct for anything that must separate two runs' entries. It is wrong for a credential, which must not, or a shared cache quietly becomes a private one per API key.

A credential belongs in a provider's own `{{ env.X }}` template, which is withheld from the key. It must **never** be routed through a case var: vars are resolved long before a provider renders anything, so the value reaches the request in the clear, is keyed, is stored on the entry, and is published in `CaseResult.vars`. The full split is in [caching.md](../concepts/caching.md#which-env-syntax).

///

### Example 44 — a second provider answers when the first refuses

```yaml
--8<-- "examples/44-provider-fallback/domarinn.yaml"
```

---

## See also

- **[protocol.md](protocol.md)** — the exec JSON protocol wire format for `exec` providers, asserts, and generators.
- **[caching.md](../concepts/caching.md)** — the one key rule, every cache knob in one table, and when `cache_salt` is needed.
- **[assertions.md](assertions.md)** — how provider outputs are graded, and the budget assertions that read `usage` / `cost_usd`.
- **[grading.md](../concepts/grading.md)** — using `anthropic` / `openai` providers as the LLM-rubric grader.
- **[domarinn.yaml](domarinn-yaml.md)** — the full suite schema (`prompts`, `tests`, `defaults`, `runner`, `cache`).
