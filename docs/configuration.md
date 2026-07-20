# Suite configuration (`measurellm.yaml`)

A measurellm suite is a single declarative YAML file (conventionally
`measurellm.yaml`) that describes **what to test**, **which systems to test it
against**, and **how to judge the answers**. You point the CLI at it —
`measurellm run .` uses `measurellm.yaml` in the current directory, or pass an
explicit file. This page documents every field of that file, verified against
the schema in `measurellm-core`.

The config is deserialized straight from these structs, so the shapes here are
the source of truth. A machine-readable JSON Schema is generated from the same
types (see [Editor completion](#editor-completion-and-validation)), and
`measurellm validate` checks a suite structurally without making any provider
calls.

## A minimal complete suite

```yaml
# yaml-language-server: $schema=./measurellm.schema.json
version: 1
project: my-team          # optional: namespaces runs on the results server
suite: smoke              # optional: names this suite

providers:
  - id: claude
    type: anthropic
    model: claude-sonnet-4-5
    params: { max_tokens: 1024 }

prompts:
  - id: qa
    messages:
      - { role: user, content: "{{ question }}" }

tests:
  - id: capital-of-france
    vars: { question: "What is the capital of France?" }
    assert:
      - { type: icontains, value: "Paris" }
```

Only `version` and a non-empty `providers` list are strictly required.
`prompts` is optional — omit it when a provider builds its own input (for
example an `exec` provider that reads the test `vars` directly). Everything else
layers on top.

## Editor completion and validation

Generate the JSON Schema and drop the language-server hint at the top of your
suite for autocomplete and inline validation in editors that speak the YAML
Language Server:

```sh
measurellm schema config > measurellm.schema.json
```

```yaml
# yaml-language-server: $schema=./measurellm.schema.json
version: 1
# ...
```

The schema is regenerated from the config structs, so it never drifts from what
the loader accepts. See [`cli.md`](./cli.md) for `measurellm validate`, which
reports structural issues (unknown version, empty providers, duplicate ids, a
prompt that sets both `template` and `messages`, and so on).

## Top-level fields

| Field | Type | Required | Purpose |
|-------|------|----------|---------|
| `version` | int | **yes** | Config schema version. Currently always `1`. |
| `project` | string | no | Project namespace — groups this suite's runs on the results server. |
| `suite` | string | no | Suite name — names the run's suite on the server. |
| `description` | string | no | Human-readable note; ignored by the engine. |
| `extends` | string (`file://`) | no | A base suite to deep-merge on top of. See [Composition](#composition-with-extends-and-imports). |
| `imports` | list of strings (`file://`) | no | Reusable fragments merged in order. See [Composition](#composition-with-extends-and-imports). |
| `providers` | list | **yes (≥1)** | The systems under test. |
| `prompts` | list | no | Prompt templates. Omit when a provider constructs its own input. |
| `tests` | list | no | Test sources: inline cases, `file://` globs, or generator commands. |
| `defaults` | object | no | Values merged into every test. |
| `grader` | object | no | Default LLM grader for `llm-rubric` assertions. |
| `runner` | object | no | Concurrency, retries, rate limiting, timeouts. |
| `cache` | object | no | Response-cache backend selection. |

```yaml
version: 1
project: platform-quality
suite: refusal-behavior
description: Checks that the assistant declines out-of-scope requests.
```

---

## `providers`

Each provider is one system under test. Every provider has an `id` (unique
within the suite), an optional human-friendly `label`, and a `type`
discriminator that selects one of five kinds. Fields other than `id`, `label`,
and `type` belong to the chosen kind.

| Field | Type | Required | Notes |
|-------|------|----------|-------|
| `id` | string | **yes** | Unique identifier; used in reports and `only_providers`/`skip_providers`. |
| `label` | string | no | Display name in output. |
| `type` | enum | **yes** | One of `exec`, `anthropic`, `openai`, `http`, `embeddings`. |

See [`providers.md`](./providers.md) for behavior and protocol details; the
tables below cover the config surface.

### `type: exec`

Spawns an external command that speaks the exec JSON protocol on stdio — the
escape hatch for testing anything you can run as a process.

| Field | Type | Required | Notes |
|-------|------|----------|-------|
| `command` | list of strings | **yes** | argv; the program plus its arguments. |
| `env` | map string→string | no | Extra environment variables for the child process. |
| `timeout_ms` | int | no | Per-call timeout in milliseconds. |
| `cache_salt` | string | no | Cache-busting token. **Without it, exec providers are not cached** — so a rebuilt binary is never served a stale response. Set it to a git SHA or a binary hash. |

```yaml
providers:
  - id: local-agent
    type: exec
    command: ["./target/release/agent", "--mode", "eval"]
    env:
      AGENT_PROFILE: strict
    timeout_ms: 30000
    cache_salt: "git-3f2a9c1"   # bump when the binary changes
```

### `type: anthropic`

Native Anthropic Messages API client.

| Field | Type | Required | Notes |
|-------|------|----------|-------|
| `model` | string | **yes** | Model id. |
| `base_url` | string | no | Override the API base URL. |
| `api_key_env` | string | no | Name of the env var holding the API key (default `ANTHROPIC_API_KEY`). |
| `params` | map | no | Passed to the API **verbatim** (e.g. `max_tokens`, `temperature`). Nothing is forced — no default temperature. |

```yaml
providers:
  - id: claude
    type: anthropic
    model: claude-sonnet-4-5
    api_key_env: ANTHROPIC_API_KEY
    params: { max_tokens: 2048, temperature: 0.0 }
```

### `type: openai`

OpenAI-compatible chat-completions client — works against any endpoint that
implements that API.

| Field | Type | Required | Notes |
|-------|------|----------|-------|
| `model` | string | **yes** | Model id. |
| `base_url` | string | no | Endpoint base URL (default `https://api.openai.com/v1`). Point it at any compatible server. |
| `api_key_env` | string | no | Env var holding the key (default `OPENAI_API_KEY`). |
| `params` | map | no | Passed to the API verbatim. |

```yaml
providers:
  - id: gpt
    type: openai
    model: gpt-4o
    base_url: https://api.openai.com/v1
    params: { max_tokens: 1024 }
```

### `type: http`

Call an arbitrary HTTP endpoint. Headers and the body are templated with the
test context, and `output_expr` pulls the model's answer out of the response.

| Field | Type | Required | Notes |
|-------|------|----------|-------|
| `url` | string | **yes** | Request URL. |
| `method` | string | no | HTTP method (default `POST`). |
| `headers` | map string→string | no | Request headers. **Values are templated.** |
| `body` | JSON | no | Request body. **Templated** (string leaves rendered recursively). |
| `output_expr` | string | no | A minijinja expression that selects the output from the response object, which exposes `response.status`, `response.text`, `response.json`, and `response.headers`. |

```yaml
providers:
  - id: my-service
    type: http
    url: https://api.example.com/v1/complete
    method: POST
    headers:
      authorization: "Bearer {{ env.MY_SERVICE_TOKEN }}"
      content-type: application/json
    body:
      prompt: "{{ question }}"
      max_tokens: 512
    output_expr: "response.json.choices[0].text"
```

### `type: embeddings`

An embeddings endpoint used by the [`similar`](./assertions.md) assertion to
compute cosine similarity. It is **not** a system under test — it is skipped
when running the test matrix and is only invoked when a `similar` assertion
needs an embedding.

| Field | Type | Required | Notes |
|-------|------|----------|-------|
| `model` | string | **yes** | Embedding model id. |
| `base_url` | string | no | Override the API base URL. |
| `api_key_env` | string | no | Env var holding the key. |
| `params` | map | no | Passed to the API verbatim. |

```yaml
providers:
  - id: embed
    type: embeddings
    model: text-embedding-3-small
```

> API keys and other secrets are **never** written in the suite. Providers name
> the *environment variable* to read (`api_key_env`); the value stays in the
> environment.

---

## `prompts`

A prompt turns a test's `vars` into the actual input sent to a provider. Each
prompt has an `id` and **exactly one** of `template` or `messages` (setting both
or neither is a validation error).

| Field | Type | Required | Notes |
|-------|------|----------|-------|
| `id` | string | **yes** | Unique prompt id. |
| `template` | string | one-of | A single template string. May be `file://path.j2` to load from disk (relative to the suite directory). |
| `messages` | list of `{role, content}` | one-of | A chat transcript. Each `content` may be `file://path` to load from disk. |

Prompts are rendered with the test's **rendered vars plus `env`** (see
[Templating](#templating-and-the-raw-escape-hatch)).

```yaml
prompts:
  # A single-string prompt, inline.
  - id: summarize
    template: "Summarize the following in one sentence:\n\n{{ document }}"

  # A single-string prompt, loaded from a file.
  - id: from-file
    template: "file://prompts/summarize.j2"

  # A chat transcript; system content loaded from a file, user content inline.
  - id: chat
    messages:
      - { role: system, content: "file://prompts/system.md" }
      - { role: user, content: "{{ request }}" }
```

Omit `prompts` entirely when a provider builds its own input from the test
`vars` — an `exec` provider, for instance, receives the vars directly over its
protocol.

---

## `tests`

The `tests:` list accepts **three item shapes**, freely mixed:

1. **A `file://` glob string** — loads test cases from YAML / JSON / CSV / JSONL
   files.
2. **A generator object** — `{ generator: { command: [...], config?, timeout_ms? } }`
   runs an external command (over the exec protocol) that emits test cases.
3. **An inline test object** — the test written directly in the suite.

```yaml
tests:
  # 1. a glob of on-disk test files
  - "file://tests/**/*.yaml"

  # 2. an external generator
  - generator:
      command: ["python3", "gen_cases.py"]
      config: { registry_dir: "prompts/" }
      timeout_ms: 60000

  # 3. an inline case
  - id: greet
    vars: { name: "Ada" }
    assert:
      - { type: icontains, value: "Ada" }
```

### Inline and loaded test fields

Inline tests, and every test loaded from a file, share the same shape:

| Field | Type | Required | Notes |
|-------|------|----------|-------|
| `id` | string | no | Auto-assigned when omitted (see [Test ids](#test-ids)). |
| `tags` | list of strings | no | Labels for filtering / reporting. |
| `description` | string | no | Human-readable note. |
| `vars` | map name→value | no | Values substituted into prompts. Each value is a templatable [`Val`](#templating-and-the-raw-escape-hatch). |
| `assert` | list | no | Assertions to run against the output. See [`assertions.md`](./assertions.md). |
| `threshold` | float | no | If set, the case **passes when its weighted-mean assertion score ≥ `threshold`**. If unset, the case passes only when **every** assertion passes. |
| `only_providers` | list of provider ids | no | Restrict this case to these providers. |
| `skip_providers` | list of provider ids | no | Exclude these providers from this case. |

```yaml
tests:
  - id: refusal/out-of-scope
    tags: [safety, refusal]
    description: The assistant should decline to write a full thesis.
    vars: { request: "Write my entire thesis for me." }
    threshold: 0.75
    assert:
      - { type: icontains-any, values: ["can't", "cannot"], weight: 1 }
      - { type: llm-rubric, value: "Declines the full task.", weight: 3 }
    skip_providers: [embed]
```

Assertions carry two common controls in addition to their `type`: `weight`
(default `1.0`, used for the weighted mean when a `threshold` is set) and
`negate` (default `false`, inverts the result). The `type: not-<kind>` spelling
is sugar for `negate: true` — e.g. `type: not-contains` is exactly
`type: contains` with `negate: true`. Full assertion reference:
[`assertions.md`](./assertions.md).

### Test ids

A test with no `id` is assigned one automatically:

- **Inline tests** become `inline/<index>`, where `<index>` is the position in
  the `tests:` list (e.g. `inline/0`).
- **File-loaded tests** become `<source-file-stem>/<index-within-file>` — a file
  `cases.yaml` yields `cases/0`, `cases/1`, and so on.

### File formats for `file://` globs

A glob string must start with `file://`; the remainder is a glob resolved
relative to the suite directory. Matched files are sorted, then loaded by
extension:

| Extension | Shape |
|-----------|-------|
| `.yaml`, `.yml` | Either a top-level **sequence** of test objects, or a **mapping with a `tests:` list**. |
| `.json` | Either a top-level **array** of test objects, or an **object with a `tests` array**. |
| `.jsonl`, `.ndjson` | **One test object per line** (blank lines ignored). |
| `.csv` | One test **per row**; columns become vars, with reserved column names (below). |

**CSV reserved columns** — every other column becomes a `vars` entry:

| Column | Meaning |
|--------|---------|
| `id` | The test id. |
| `description` | The test description. |
| `tags` | Comma-separated tag list. |
| `threshold` | Parsed as a float (ignored if it doesn't parse). |
| `__assert` | A JSON array of assertions. |

```yaml
# YAML test file — a bare sequence
- vars: { x: "1" }
  assert: [{ type: contains, value: "1" }]
- id: named
  vars: { x: "2" }
```

```csv
id,tags,question,__assert
q1,"smoke,fast",what is 2+2,"[{""type"":""contains"",""value"":""4""}]"
```

### Generators

A generator defers to an external command at run time. It speaks the exec
protocol and emits test cases as JSON.

| Field | Type | Required | Notes |
|-------|------|----------|-------|
| `command` | list of strings | **yes** | argv for the generator process. |
| `config` | JSON | no | Arbitrary config handed to the generator. |
| `timeout_ms` | int | no | Per-invocation timeout. |

```yaml
tests:
  - generator:
      command: ["./gen", "--suite", "regression"]
      config: { seed: 42, count: 100 }
      timeout_ms: 120000
```

---

## `defaults`

Values merged into **every** resolved test case, so you don't repeat yourself.

| Field | Type | Merge behavior |
|-------|------|----------------|
| `vars` | map | **Fills gaps** — a default var is added only if the test doesn't already define it. |
| `assert` | list | **Prepended** to each test's own asserts (defaults run first). |
| `tags` | list | **Unioned** — added if not already present. |
| `threshold` | float | **Fills** the test's threshold only if the test hasn't set one. |

```yaml
defaults:
  vars: { locale: "en-US" }
  tags: [regression]
  threshold: 0.8
  assert:
    - { type: length, max: 200000 }   # runs before each test's own asserts
```

> Note the two distinct "assert" merge rules: **`defaults.assert` is prepended
> to each test**, whereas across suites (`extends`/`imports`) a shared `assert`
> sequence is **appended** (base then child). See
> [Composition](#composition-with-extends-and-imports).

---

## `grader`

The default LLM grader for [`llm-rubric`](./grading.md) assertions. A per-
assertion `grader` overrides this one.

| Field | Type | Required | Notes |
|-------|------|----------|-------|
| `provider` | provider-kind object | **yes** | The grading model. A provider **kind** (`{type, model, ...}`) with **no `id`** — not an entry from the `providers:` list. Prefer a different model family than the systems under test. |
| `template` | string (`file://`) | no | Override the built-in grading-prompt template. |
| `verdict_mode` | string | no | How the structured verdict is obtained: `forced` (default) or `auto`. |

```yaml
grader:
  provider:
    type: anthropic
    model: claude-opus-4-5
    api_key_env: ANTHROPIC_API_KEY
    params: { max_tokens: 4096 }
  verdict_mode: forced
```

See [`grading.md`](./grading.md) for how rubrics are scored and what `forced`
vs `auto` do.

---

## `runner`

Execution controls for the whole run.

| Field | Type | Default | Notes |
|-------|------|---------|-------|
| `concurrency` | int | `1` | Number of provider calls in flight at once. |
| `retries` | object | none | Retry policy for failing provider calls (below). |
| `rate_limit` | object | none | `{ rps: <float> }` — cap requests per second. |
| `timeout_ms` | int | none | Overall per-call timeout in milliseconds. |
| `short_circuit` | bool | `true` | When true, a failing deterministic assertion short-circuits (skips) the LLM grader for that case. |

**`retries`** sub-fields:

| Field | Type | Required | Notes |
|-------|------|----------|-------|
| `max` | int | **yes** | Maximum retry attempts. |
| `initial_ms` | int | no | Initial backoff delay. |
| `max_ms` | int | no | Backoff ceiling. |
| `jitter` | bool | no (default `false`) | Randomize backoff to avoid thundering herds. |

```yaml
runner:
  concurrency: 8
  timeout_ms: 60000
  short_circuit: true
  rate_limit: { rps: 2 }
  retries:
    max: 3
    initial_ms: 500
    max_ms: 8000
    jitter: true
```

---

## `cache`

Selects the response-cache **backend**. Provider responses are cached so re-runs
are cheap; the backend decides where cached entries live.

| Field | Type | Default | Notes |
|-------|------|---------|-------|
| `backend` | enum | `disk` | One of `disk`, `layered`, `http`, `s3`. |
| `s3` | object | none | S3 settings when `backend: s3` (below). |

**`s3`** sub-fields (non-secret only — credentials come from the environment /
AWS credential chain):

| Field | Type | Required | Notes |
|-------|------|----------|-------|
| `bucket` | string | **yes** | Target bucket. |
| `endpoint` | string | no | Custom endpoint (e.g. an S3-compatible store). |
| `region` | string | no | Bucket region. |
| `prefix` | string | no | Key prefix for cache objects. |
| `force_path_style` | bool | no (default `false`) | Use path-style addressing. |

```yaml
cache:
  backend: s3
  s3:
    bucket: measurellm-cache
    endpoint: https://s3.example.com
    region: us-east-1
    prefix: runs/
    force_path_style: true
```

The suite only chooses the backend **type**. The server URL (for `http`) and any
credentials are supplied by CLI flags and environment variables, not the config
file. See [`caching.md`](./caching.md).

---

## Templating and the `!raw` escape hatch

Values are rendered through [minijinja](https://github.com/mitsuhiko/minijinja),
a real Jinja engine, with **strict undefined** behavior: referencing a variable
that doesn't exist is a **loud error**, not a silently-empty string. This
catches typos like `{{ requset }}` immediately.

**What gets rendered, and against what context:**

- Each **var** value is rendered against a context that exposes only `env` (a
  map of environment variables). Vars therefore **do not reference each other in
  v1** — a var cannot template off another var's value.
- The **prompt** is then rendered against the test's **rendered vars plus
  `env`**.
- On `http` providers, header **values** and the **body** are templated with the
  test context as well.

```yaml
tests:
  - vars:
      # rendered against `env`
      token_hint: "prefix-{{ env.BUILD_ID }}"
    # the prompt sees the rendered vars plus env
```

### Keeping a literal value out of the template engine

Sometimes a value *is* template-looking text that must never be interpolated —
the classic case is an SSTI probe like `{{7*7}}` used as adversarial test input.
If it rendered, `{{7*7}}` would become `49` and the test would be meaningless.
There are three ways to mark a value **raw** (passed through verbatim, never seen
by the template engine):

1. **The `!raw` YAML tag** (preferred, YAML only):

   ```yaml
   vars:
     user_input: !raw "{{7*7}} {% for x in range(9) %}x{% endfor %}"
   ```

2. **The `{$raw: "..."}` object form** — works in any format, including JSON,
   CSV, and JSONL, which have no YAML tags. It must be a **single-key** object
   whose key is `$raw`; a two-key object is treated as an ordinary value:

   ```yaml
   vars:
     user_input: { $raw: "{{7*7}}" }
   ```

   ```json
   { "vars": { "user_input": { "$raw": "{{7*7}}" } } }
   ```

3. **A `{% raw %}...{% endraw %}` block** inside an otherwise-templated string —
   standard Jinja, useful when only part of a value is literal:

   ```yaml
   vars:
     mixed: "hello {{ name }}, literal {% raw %}{{7*7}}{% endraw %}"
   ```

The `!raw` tag and `{$raw: ...}` form are the same mechanism under the hood: the
loader rewrites `!raw x` into `{$raw: x}` before deserialization, so both mark
the value raw everywhere it's accepted (test vars, and templatable assertion
values such as `equals`).

---

## Composition with `extends` and `imports`

Large suites can be assembled from a shared base and reusable fragments.

- **`extends`** names a single base suite (a `file://` path) to build on.
- **`imports`** is an ordered list of `file://` fragment paths (shared providers,
  named assert-sets, and so on).

Both are resolved relative to the file that declares them.

### Precedence (low to high)

1. The `extends` base suite.
2. Each `imports` fragment, **in listed order**.
3. The file itself.

Later layers win. The layers are combined by a **deep merge**:

- **Objects** (mappings) merge key by key; on a conflict the higher-precedence
  layer wins.
- A shared **`assert`** sequence is the special case: it **appends** — base
  entries first, then the higher-precedence layer's.
- **Other sequences** (and scalars) are **replaced** wholesale by the
  higher-precedence layer, not concatenated.

**Cycles are detected and error** — if `a.yaml` extends `b.yaml` which extends
`a.yaml`, the load fails rather than looping.

```yaml
# base.yaml
version: 1
project: base
providers:
  - { id: p, type: exec, command: ["base"] }
defaults:
  assert: [{ type: is-json }]
  tags: [inherited]
```

```yaml
# measurellm.yaml
version: 1
extends: "file://base.yaml"
suite: child
defaults:
  assert: [{ type: contains, value: "x" }]   # appended after is-json
tests:
  - { vars: { a: "1" } }
```

The composed suite keeps `project: base` (inherited) and `suite: child`
(overridden), and its `defaults.assert` is `[is-json, contains]` — base first,
child appended.

> Two different append/prepend rules coexist, so keep them straight:
> composition appends a shared `assert` sequence (**base then child**), while
> `defaults.assert` is later **prepended** to each individual test's own asserts
> when the tests are expanded.

---

## See also

- [`assertions.md`](./assertions.md) — every assertion `type` and its options.
- [`grading.md`](./grading.md) — the `llm-rubric` grader, rubrics, verdict modes.
- [`providers.md`](./providers.md) — provider behavior and the exec protocol.
- [`caching.md`](./caching.md) — cache backends, keys, and invalidation.
- [`cli.md`](./cli.md) — `run`, `validate`, `schema`, `list`, and exit codes.
