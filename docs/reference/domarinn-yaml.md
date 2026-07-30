<!-- Canonicality: domarinn-yaml.md documents the suite file's shape — every top-level
key, once. Any key whose meaning varies by provider type is documented once in
providers.md. If you are writing the same sentence in both places, it belongs in
providers.md. -->

# domarinn.yaml

A domarinn suite is a single declarative YAML file (conventionally `domarinn.yaml`) that describes **what to test**, **which systems to test it against**, and **how to judge the answers**. You point the CLI at it — `domarinn run .` uses `domarinn.yaml` in the current directory, or pass an explicit file. This page documents every field of that file, verified against the schema in `domarinn-core`.

The config is deserialized straight from these structs, so the shapes here are the source of truth. A machine-readable JSON Schema is generated from the same types (see [Editor completion](#editor-completion-and-validation)), and `domarinn validate` checks a suite structurally without making any provider calls.

## A minimal complete suite

```yaml
# yaml-language-server: $schema=./domarinn.schema.json
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

Only `version` and a non-empty `providers` list are strictly required. `prompts` is optional — omit it when a provider builds its own input (for example an `exec` provider that reads the test `vars` directly). Everything else layers on top.

## Editor completion and validation

Generate the JSON Schema and drop the language-server hint at the top of your suite for autocomplete and inline validation in editors that speak the YAML Language Server:

```sh
domarinn schema config > domarinn.schema.json
```

```yaml
# yaml-language-server: $schema=./domarinn.schema.json
version: 1
# ...
```

The schema is regenerated from the config structs, so it never drifts from what the loader accepts. See [`cli.md`](cli.md) for `domarinn validate`, which reports structural issues (unknown version, empty providers, duplicate ids, a prompt that sets both `template` and `messages`, and so on).

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
| `tools` | list | no | Tools every provider may offer the model. See [`tools`](#tools). |
| `tests` | list | no | Test sources: inline cases, `file://` globs, or generator commands. |
| `defaults` | object | no | Values merged into every test. |
| `grader` | object | no | Default LLM grader for `llm-rubric` assertions. |
| `runner` | object | no | Concurrency, retries, rate limiting, timeouts. |
| `cache` | object | no | Cache backend selection. |

```yaml
version: 1
project: platform-quality
suite: refusal-behavior
description: Checks that the assistant declines out-of-scope requests.
```

---

## `providers`

Each provider is one system under test. Every provider has an `id` (unique within the suite), an optional human-friendly `label`, and a `type` discriminator that selects one of five kinds. Fields other than `id`, `label`, and `type` belong to the chosen kind — and because their meaning varies by `type`, they are documented once, in [`providers.md`](providers.md), rather than here.

| Field | Type | Required | Notes |
|-------|------|----------|-------|
| `id` | string | **yes** | Unique identifier; used in reports and `only_providers`/`skip_providers`. |
| `label` | string | no | Display name in output. |
| `type` | enum | **yes** | One of the five kinds below. |

| `type` | Selects |
|--------|---------|
| [`exec`](providers.md#exec) | An external command speaking the exec JSON protocol — the escape hatch for testing anything you can run as a process. |
| [`anthropic`](providers.md#anthropic) | A native Anthropic Messages API client. |
| [`openai`](providers.md#openai) | An OpenAI-compatible chat-completions client — OpenAI itself, or any compatible gateway. |
| [`http`](providers.md#http) | An arbitrary HTTP endpoint, templated with the test context. |
| [`embeddings`](providers.md#embeddings) | An embeddings endpoint that powers the [`similar`](assertions.md#similar) assertion. **Not** a system under test. |

> API keys and other secrets are **never** written in the suite. Every provider
> type names the *environment variable* to read (`api_key_env`); the value
> stays in the environment and is read at call time, never interpolated.

### Environment interpolation (`${env:VAR}`)

Provider **configuration** may pull values from the environment with a `${env:VAR}` placeholder, resolved once at load time — so a suite can point at a per-developer endpoint or a per-environment gateway without committing it:

```yaml
providers:
  - id: gateway
    type: openai
    model: "${env:LLM_MODEL}"
    base_url: "${env:LLM_BASE_URL:-https://api.openai.com/v1}"   # with a default
    api_key_env: LLM_API_KEY   # still just the *name* of the key's env var
```

| Form | Meaning |
|------|---------|
| `${env:VAR}` | The value of `VAR`. **An unset `VAR` is a hard load error** that names the field and the variable. |
| `${env:VAR:-default}` | The value of `VAR`, or `default` when `VAR` is unset. |
| `$${env:VAR}` | An **escape** — rendered as the literal text `${env:VAR}`, no lookup. |

Scope is deliberately narrow. Interpolation runs **only** over provider-bearing config: every entry in `providers`, the `provider:` block inside any `grader` (top-level or on an `llm-rubric` assert), and the `cache.s3` settings. It is resolved on the fully composed document — after [`extends`/`imports`](#composition-with-extends-and-imports) merge — so a placeholder introduced by a base layer still resolves.

`${env:…}` is **not** applied to test `vars`. Vars have their own, sandboxed templating (`{{ env.X }}`, below), kept separate so an untrusted fixture can never smuggle a different endpoint or key name into a provider. A `${…}` that is not `${env:…}` (for example a placeholder your own HTTP backend interprets) is left untouched.

> This resolves the *endpoint*, not the *secret*. API keys are still read at
> call time from the variable named by `api_key_env` — never written into the
> suite, and never interpolated here.

---

## `prompts`

A prompt turns a test's `vars` into the actual input sent to a provider. Each prompt has an `id` and **exactly one** of `template` or `messages` (setting both or neither is a validation error).

| Field | Type | Required | Notes |
|-------|------|----------|-------|
| `id` | string | **yes** | Unique prompt id. |
| `template` | string | one-of | A single template string. May be `file://path.j2` to load from disk (relative to the suite directory). |
| `messages` | list of `{role, content}` | one-of | A chat transcript. Each `content` may be `file://path` to load from disk. |

Prompts are rendered with the test's **rendered vars plus `env`** (see [Templating](#templating-and-the-raw-escape-hatch)).

```yaml
prompts:
  # A single-string prompt, inline.
  - id: summarize
    template: "Summarize the following in one sentence:\n\n{{ document }}"

  # A single-string prompt, loaded from a file.
  - id: from-file
    template: "file://prompts/summarize.j2"
```

A `messages:` prompt is a full transcript, not one string — system, a prior user turn, a prior assistant turn, then the newest one. Every turn is a template; a fixed turn simply has nothing in it to substitute. Real history, from a shipped, tested example (see [example 34](../examples/templates-and-test-data.md#example-34--a-multi-turn-conversation)):

```yaml
--8<-- "examples/34-multi-turn-conversation/domarinn.yaml:messages"
```

The provider sees the same array on every call; it is domarinn that fans a case out over `vars`, one rendering per case, not the shape of the messages.

Omit `prompts` entirely when a provider builds its own input from the test `vars` — an `exec` provider, for instance, receives the vars directly over its protocol.

---

## `tests`

The `tests:` list accepts **three item shapes**, freely mixed:

1. **A `file://` glob string** — loads tests from YAML / JSON / CSV / JSONL files.
2. **A generator object** — `{ generator: { command: [...], config?, timeout_ms? } }` runs an external command (over the exec protocol) that emits tests.
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
| `matrix` | map name→list | no | Parameter sweep: the case fans out over the cartesian product of its axes. See [Matrix / parameter sweeps](#matrix--parameter-sweeps). |
| `matrix_id` | string | no | minijinja template for a matrix cell's id, rendered against the axis values. See [Matrix / parameter sweeps](#matrix--parameter-sweeps). |
| `assert` | list | no | Assertions to run against the output. See [`assertions.md`](assertions.md). |
| `threshold` | float | no | If set, the case **passes when its weighted-mean assertion score ≥ `threshold`**. If unset, the case passes only when **every** assertion passes. |
| `cache_salt` | string | no | Opaque per-case cache-busting token, folded into the key of **this case's** provider requests only. Never sent to the provider and never templated. Use it when the system under test loads content domarinn cannot see. See [caching.md](../concepts/caching.md#per-case-salts). |
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

Assertions carry two common controls in addition to their `type`: `weight` (default `1.0`, used for the weighted mean when a `threshold` is set) and `negate` (default `false`, inverts the result). The `type: not-<kind>` spelling is sugar for `negate: true` — e.g. `type: not-contains` is exactly `type: contains` with `negate: true`. Full assertion reference: [`assertions.md`](assertions.md).

### Matrix / parameter sweeps

A case with a `matrix` **fans out over the cartesian product of its axes** — one concrete case per combination. Each axis maps a var name to the list of values it takes, and each combination's axis values are merged into `vars` (the axis **wins** over a base var of the same name, for that key only):

```yaml
--8<-- "examples/08-matrix-sweeps/domarinn.yaml:matrix"
```

The `greet` case expands to **four** cases: a 2×2 sweep over `style` and `temperature`. Axes iterate in **sorted key order**, and the default id encodes every `key=value` pair, so ids are deterministic and stable across runs (which keeps [`CaseKey`](#test-ids)s — and therefore diffing — stable):

```
greet[style=terse,temperature=0]
greet[style=terse,temperature=1]
greet[style=warm,temperature=0]
greet[style=warm,temperature=1]
```

The `locale` case shows the friendlier id shape: `matrix_id` is a minijinja template rendered against each combination's axis values, so it expands to `sweep-en`, `sweep-fr`, `sweep-de` instead of the default `locale[lang=en]` form.

Notes:

- The matrix expands **before** `defaults` are merged, and it applies to file-loaded cases too — a `matrix` in a `.yaml`/`.json` test file sweeps just like an inline one.
- An **empty axis** (`style: []`) is an error — there is nothing to sweep.
- Expansion is pure: a `!raw` axis value stays raw in the produced case's vars.
- `matrix_id` sees only the axis values (not base vars), so it stays deterministic.

### Test ids

A test with no `id` is assigned one automatically:

- **Inline tests** become `inline/<index>`, where `<index>` is the position in the `tests:` list (e.g. `inline/0`).
- **File-loaded tests** become `<source-file-stem>/<index-within-file>` — a file `cases.yaml` yields `cases/0`, `cases/1`, and so on.
- **Generator-produced tests** become `<command-stem>/<index>`, where the stem comes from the **first argv element**, not from any file the generator reads. `command: ["./gen-cases.py", "--all"]` yields `gen-cases/0`; `command: ["sh", "-c", "..."]` yields `sh/0`, because `sh` is what was spawned.

That last rule is worth knowing before you migrate a `file://` glob to a generator: the ids all change, and every case collapses into a single group, so `--filter` patterns and per-provider baselines written against the old ids stop matching. Have the generator emit its own `id` on each case if you want ids that survive the move.

`domarinn list tests --generators` runs the generators and prints the ids they produce, which is how you preview `--filter` targets on a generator-driven suite. See [cli.md](cli.md).

### File formats for `file://` globs

A glob string must start with `file://`; the remainder is a glob resolved relative to the suite directory. Matched files are sorted, then loaded by extension:

| Extension | Shape |
|-----------|-------|
| `.yaml`, `.yml` | Either a top-level **sequence** of test objects, or a **mapping with a `tests:` list**. |
| `.json` | Either a top-level **array** of test objects, or an **object with a `tests` array**. |
| `.jsonl`, `.ndjson` | **One test object per line** (blank lines ignored). |
| `.csv` | One test **per row**; columns become vars, with reserved column names (below). |
| `.tsv` | Same as `.csv`, but **tab-separated**. |

**CSV reserved columns** — every other column becomes a `vars` entry:

| Column | Meaning |
|--------|---------|
| `id` | The test id. |
| `description` | The test description. |
| `tags` | Comma-separated tag list. |
| `threshold` | Parsed as a float (ignored if it doesn't parse). |
| `cache_salt` | The case's [cache salt](../concepts/caching.md#per-case-salts) — reserved so a digest column keys the cache instead of becoming a var. |
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

A `.tsv` file is the same as `.csv` but **tab-separated** — handy when field values contain commas.

### File-content vars (`!file` / `{$file}`)

A single var can pull its value from a file next to the suite instead of being written inline — useful for large documents, shared golden fixtures, and adversarial inputs kept out of the YAML:

```yaml
tests:
  - vars:
      document: !file "fixtures/article.txt"          # loaded as text
      rubric:   { $file: "fixtures/rubric.json" }     # parsed by extension → JSON
      payload:  { $file: "fixtures/probe.txt", raw: true }  # never templated
```

The `!file "path"` YAML tag and the format-agnostic `{$file: "path"}` object form are the same mechanism (the tag desugars to the object, exactly like `!raw`), so file vars work in JSON / JSONL / CSV fixtures too.

| Option | Default | Meaning |
|--------|---------|---------|
| `$file` | — (required) | Path to the fixture, **relative to the suite directory**. |
| `parse` | `true` | When true, parse by extension: `.json` → JSON, `.yaml`/`.yml` → YAML, anything else → text. Set `false` to force text even for a structured extension. |
| `raw` | `false` | When true, the loaded content is marked [raw](#keeping-a-literal-value-out-of-the-template-engine) and is **never** run through the template engine — the right choice for untrusted fixtures (an SSTI payload in the file stays literal). |

**Sandboxed.** The path is resolved relative to the suite directory and must stay inside it: `!file "../../etc/passwd"` (or a symlink pointing outside the suite) is **refused**, not read. The same guard covers `file://` prompt/message content and `file://` test-file globs.

**Cache note.** A file var's content is rendered into the request — into the prompt for a vendor provider, into the `vars` an `exec` child receives, into the url/headers/body of an `http` call — and [the request is the cache key](../concepts/caching.md#the-rule). So **editing a fixture busts the cache** for the cases that read it, exactly as editing an inline value would.

### Generators

A generator defers to an external command at run time. It speaks the exec protocol and emits tests as JSON.

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

## `tools`

Tools every provider in this suite may offer the model, graded by [`tool-call`](assertions.md#tool-call) assertions.

| Field | Type | Required | Notes |
|-------|------|----------|-------|
| `name` | string | **yes** | The tool's name, as the model will see it. |
| `description` | string | no | What it does. |
| `input_schema` | JSON Schema | no | The arguments' shape. Passed to the provider verbatim and never templated. |

```yaml
--8<-- "examples/15-tool-call-asserts/domarinn.yaml:tools"
```

Declaring a tool does **not** make domarinn run one — it never executes a tool and never feeds a result back. What it wants is the model's *decision*, which is fully observable in a single turn. See [protocol.md](protocol.md#tools).

Suite-level rather than per-test on purpose: the tool surface is a property of the system being evaluated, and varying it per case would make two cases incomparable while still looking like one suite.

Supported by `exec`, `anthropic` and `openai` providers. The declarations reach each vendor in its own shape (OpenAI wraps them as `function` objects), and are part of the request, so they are part of the cache key — a call offering a tool and one that does not are different questions.

---

## `defaults`

Values merged into **every** resolved test, so you don't repeat yourself.

| Field | Type | Merge behavior |
|-------|------|----------------|
| `vars` | map | **Fills gaps** — a default var is added only if the test doesn't already define it. |
| `assert` | list | **Prepended** to each test's own asserts (defaults run first). |
| `tags` | list | **Unioned** — added if not already present. |
| `threshold` | float | **Fills** the test's threshold only if the test hasn't set one. |
| `cache_salt` | string | **Fills** the test's [cache salt](../concepts/caching.md#per-case-salts) only if the test hasn't set one — generator-produced cases included. |

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

Defaults are merged into generator-produced cases too. Those cases do **not** get matrix expansion or `file://` var resolution, which both run before a generator has produced anything.

---

## `grader`

The default LLM grader for [`llm-rubric`](../concepts/grading.md) assertions. A per- assertion `grader` overrides this one.

| Field | Type | Required | Notes |
|-------|------|----------|-------|
| `provider` | provider-kind object | **yes** | The grading model. A provider **kind** (`{type, model, ...}`) with **no `id`** — not an entry from the `providers:` list. Prefer a different model family than the systems under test. |
| `template` | string (`file://`) | no | Override the built-in grading-prompt template. Resolved relative to the suite directory and sandboxed to it, like every other `file://`. It renders into the prompt the grader reads, and the grader's request is its cache key — so editing the grading prompt re-grades rather than replaying the old verdict. |
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

See [`grading.md`](../concepts/grading.md) for how rubrics are scored and what `forced` vs `auto` do.

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
--8<-- "examples/20-runner-tuning/domarinn.yaml:runner"
```

---

## `cache`

Selects the cache **backend**. Every outgoing request is cached so re-runs are cheap; the backend decides where entries live. What goes into a key is one rule, documented once in [caching.md](../concepts/caching.md#the-rule).

| Field | Type | Default | Notes |
|-------|------|---------|-------|
| `backend` | enum | `disk` | `disk` (local only) or `layered` (local disk in front of a shared remote). `http` and `s3` are **deprecated aliases** that warn at startup and name one tier outright instead of letting `cache.s3` choose — so they match `layered` only when the two agree (`s3` without a `cache.s3` block degrades to local disk alone). |
| `s3` | object | none | S3 settings, which are also what makes `layered` pick the S3 remote over the server (below). |
| `grader` | bool | *(unset)* | **Deprecated.** `false` disables grader-request caching for every run of this suite; accepted with a warning. Prefer the `--no-grader-cache` run flag — whether to re-grade is a property of one run, not of the suite. |

**`s3`** sub-fields (non-secret only — credentials come from the environment / AWS credential chain):

| Field | Type | Required | Notes |
|-------|------|----------|-------|
| `bucket` | string | **yes** | Target bucket. |
| `endpoint` | string | no | Custom endpoint (e.g. an S3-compatible store). |
| `region` | string | no | Bucket region. |
| `prefix` | string | no | Key prefix for cache objects. |
| `force_path_style` | bool | no (default `false`) | Use path-style addressing. |

```yaml
cache:
  backend: layered
  s3:
    bucket: domarinn-cache
    endpoint: https://s3.example.com
    region: us-east-1
    prefix: runs/
    force_path_style: true
```

The suite only chooses the backend **type**. The server URL and any credentials are supplied by CLI flags and environment variables, not the config file — which is what keeps a suite safe to commit. A remote whose URL or credentials are missing degrades to local disk with a warning rather than failing the run. See [`caching.md`](../concepts/caching.md#backends).

---

## Templating and the `!raw` escape hatch

Values are rendered through [minijinja](https://github.com/mitsuhiko/minijinja), a real Jinja engine, with **strict undefined** behavior: referencing a variable that doesn't exist is a **loud error**, not a silently-empty string. This catches typos like `{{ requset }}` immediately.

**What gets rendered, and against what context:**

- Each **var** value is rendered against a context that exposes only `env` (a map of environment variables). Vars therefore **do not reference each other in v1** — a var cannot template off another var's value.
- The **prompt** is then rendered against the test's **rendered vars plus `env`**.
- On `http` providers, header **values** and the **body** are templated with the test context as well.

```yaml
tests:
  - vars:
      # rendered against `env`
      token_hint: "prefix-{{ env.BUILD_ID }}"
    # the prompt sees the rendered vars plus env
```

### Filters and functions

On top of minijinja's own builtins — `trim`, `default`, `int`, `float`, `lower`, `upper`, `length`, `replace`, `join`, and the rest — domarinn registers the filters and functions below. They are available anywhere a template is rendered: vars, prompts, `http` headers/body, and `jinja` assertions.

**Deterministic** (same input → same output, always):

| Call | Kind | Result |
|------|------|--------|
| `x \| json_encode` / `x \| to_json` | filter | Compact JSON string of any value. |
| `s \| json_decode` / `s \| from_json` | filter | Parse a JSON string into a value. |
| `s \| b64encode` / `s \| b64decode` | filter | Standard (padded) base64 encode/decode. |
| `s \| sha256` | filter | Lower-case hex SHA-256 digest. |
| `s \| md5` | filter | Lower-case hex MD5 digest (legacy interop; not for security). |
| `s \| blake3` | filter | Lower-case hex BLAKE3 digest. |
| `s \| regex_replace(pat, repl)` | filter | Replace every match of `pat`; `$1` backreferences work in `repl`. |
| `s \| regex_match(pat)` | filter | `true` if `pat` matches anywhere in `s`. |
| `s \| slugify` | filter | ASCII slug: lower-cased, non-alphanumerics collapse to single `-`, no leading/trailing dashes. |
| `s \| truncate(n)` | filter | First `n` characters (by Unicode scalar). |

**Pinned non-deterministic** — because a rendered prompt is persisted with the run and diffed across runs, these are constrained so a render stays reproducible:

| Call | Result |
|------|--------|
| `now()` / `now(fmt)` | Current time as RFC3339, or `strftime`-formatted. Honors `DOMARINN_NOW` (an RFC3339 instant) when set, so a run can be pinned; otherwise the wall clock. |
| `uuid(seed)` | A deterministic, v4-shaped UUID derived from `seed`. |
| `rand(seed)` | A deterministic float in `[0, 1)` derived from `seed`. |
| `randint(lo, hi, seed)` | A deterministic integer in `[lo, hi]` (inclusive). |

`uuid`, `rand`, and `randint` **require an explicit seed** — an unseeded `rand()` is a template error, not a fresh random value. Seed with something stable and per-case (e.g. `rand(vars.case_id)`) when you want variety that still reproduces.

```yaml
tests:
  - vars:
      doc_id: "{{ doc | sha256 | truncate(12) }}"
      slug: "{{ title | slugify }}"
      nonce: "{{ uuid(doc_id) }}"
```

A [raw value](#keeping-a-literal-value-out-of-the-template-engine) bypasses the engine entirely, so `{{ x | sha256 }}` inside a `!raw` value is passed through verbatim — filters never touch it.

### Keeping a literal value out of the template engine

Sometimes a value *is* template-looking text that must never be interpolated — the classic case is an SSTI probe like `{{7*7}}` used as adversarial test input. If it rendered, `{{7*7}}` would become `49` and the test would be meaningless. There are three ways to mark a value **raw** (passed through verbatim, never seen by the template engine):

1. **The `!raw` YAML tag** (preferred, YAML only):

   ```yaml
   vars:
     user_input: !raw "{{7*7}} {% for x in range(9) %}x{% endfor %}"
   ```

2. **The `{$raw: "..."}` object form** — works in any format, including JSON, CSV, and JSONL, which have no YAML tags. It must be a **single-key** object whose key is `$raw`; a two-key object is treated as an ordinary value:

   ```yaml
   vars:
     user_input: { $raw: "{{7*7}}" }
   ```

   ```json
   { "vars": { "user_input": { "$raw": "{{7*7}}" } } }
   ```

3. **A `{% raw %}...{% endraw %}` block** inside an otherwise-templated string — standard Jinja, useful when only part of a value is literal:

   ```yaml
   vars:
     mixed: "hello {{ name }}, literal {% raw %}{{7*7}}{% endraw %}"
   ```

The `!raw` tag and `{$raw: ...}` form are the same mechanism under the hood: the loader rewrites `!raw x` into `{$raw: x}` before deserialization, so both mark the value raw everywhere it's accepted (test vars, and templatable assertion values such as `equals`).

---

## Composition with `extends` and `imports`

Large suites can be assembled from a shared base and reusable fragments.

- **`extends`** names a single base suite (a `file://` path) to build on.
- **`imports`** is an ordered list of `file://` fragment paths (shared providers, named assert-sets, and so on).

Both are resolved relative to the file that declares them.

### Precedence (low to high)

1. The `extends` base suite.
2. Each `imports` fragment, **in listed order**.
3. The file itself.

Later layers win. The layers are combined by a **deep merge**:

- **Objects** (mappings) merge key by key; on a conflict the higher-precedence layer wins.
- A shared **`assert`** sequence is the special case: it **appends** — base entries first, then the higher-precedence layer's.
- **Other sequences** (and scalars) are **replaced** wholesale by the higher-precedence layer, not concatenated.

**Cycles are detected and error** — if `a.yaml` extends `b.yaml` which extends `a.yaml`, the load fails rather than looping.

```yaml
--8<-- "examples/17-defaults-and-composition/base.yaml"
```

```yaml
--8<-- "examples/17-defaults-and-composition/domarinn.yaml:extends"
```

The composed suite keeps `runner.concurrency: 4` (inherited — the child declares no `runner` of its own), overrides `defaults.vars.product` to `"Aurora Notes Pro"` (a map merge, child wins), and its `defaults.assert` is `[not-icontains "as an AI", length max 2000]` — base first, child appended.

> Two different append/prepend rules coexist, so keep them straight:
> composition appends a shared `assert` sequence (**base then child**), while
> `defaults.assert` is later **prepended** to each individual test's own asserts
> when the tests are expanded.

---

## Strict validation

A suite is validated structurally before any provider is contacted — both by `domarinn validate` and at the start of `domarinn run`. Validation is strict on purpose: a typo is an error you can fix, not a field that is silently ignored while the setting it was meant to change quietly does nothing.

- **Unknown or misspelled keys are a hard error that names the file, the dotted path, and the key.** A stray `maxx:` under `runner.retries`, for instance, fails with a message like ``examples/x/domarinn.yaml: runner.retries: unknown field `maxx` `` — so you can jump straight to the offending line.
- **The check reaches inside provider, assertion, and grader mappings.** A typo'd provider field (`basurl:` for `base_url:`) or assertion option is flagged, **including the `provider:` block nested inside a grader** — both the top-level `grader.provider` and the inner grader on any `llm-rubric` assertion (in `defaults.assert` or an inline test's `assert`). Free-form bags — a provider's `params`, an `http` provider's `body`, an `exec` assertion's `config` — are opaque values, not schema, so their inner keys are intentionally left unchecked.
- **A message `role` must be `system`, `user`, or `assistant`.** Any other value is rejected at load time.
- **An `http` provider's `method` is normalized to upper case.** You may write `method: get` or `method: GET`; the method sent on the wire — and the one the cache keys on — is always the upper-case form, so two spellings of one method share entries rather than paying twice.

---

## A complete suite, annotated

The [minimal suite](#a-minimal-complete-suite) at the top of this page is where a suite starts. The other end of the range ships as a runnable example: [Example 38 — Every key, once](../examples/models-grading-and-budgets.md#example-38--every-key-once) sets every top-level key documented above exactly once, and annotates each with what it is and where its full story lives — including the `extends` base and `imports` fragment it composes from.

Read it as a map of this page rather than as a suite to copy. It is linked rather than reproduced here so there stays one copy of those bytes — the file, the page that transcludes it, and the CI run that executes it all read the same suite. A guard in `crates/domarinn-cli/tests/examples.rs` fails if the config grows a top-level key that example stops setting, so "every key" stays true on both pages.

---

## See also

- [`assertions.md`](assertions.md) — every assertion `type` and its options.
- [`grading.md`](../concepts/grading.md) — the `llm-rubric` grader, rubrics, verdict modes.
- [`providers.md`](providers.md) — provider behavior and the exec protocol.
- [`caching.md`](../concepts/caching.md) — cache backends, keys, and invalidation.
- [`cli.md`](cli.md) — `run`, `validate`, `schema`, `list`, and exit codes.
