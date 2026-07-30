# Assertions

An **assertion** grades one provider output. A test carries a list of assertions under `assert:`; each produces a score in `[0, 1]` and a pass/fail flag, and the case's own verdict is derived from them (see [Scoring](#scoring)).

Assertions come in two families:

- **Deterministic** assertions need no network. They run **first**, in config order, so a cheap local failure can **short-circuit** the expensive graded assertions and avoid spending money.
- **Graded** assertions need an external call (a subprocess, an LLM grader, or an embeddings API). They run **after** the deterministic ones, and only if they can still change the case outcome.

> Source of truth: `crates/domarinn-core/src/asserts.rs` (deterministic
> logic), `scoring.rs` (score + short-circuit), `grader.rs` (graded logic),
> `config.rs` (the `AssertKind` schema), `runner.rs` (orchestration).

---

## Every assertion type

The `type` field selects the assertion. Names are kebab-case.

| `type`           | Family        | Config fields                          | Passes when… |
|------------------|---------------|----------------------------------------|--------------|
| `contains`       | deterministic | `value: string`                        | output contains the substring (case-sensitive) |
| `icontains`      | deterministic | `value: string`                        | output contains the substring (case-insensitive) |
| `icontains-any`  | deterministic | `values: [string]`                     | output contains **any** of the substrings (case-insensitive) |
| `regex`          | deterministic | `value: string`                        | the regex matches somewhere in the output |
| `equals`         | deterministic | `value: any` (may be `!raw`)           | output equals the (rendered) expected value |
| `starts-with`    | deterministic | `value: string`                        | output starts with the prefix |
| `is-json`        | deterministic | –                                      | the whole output parses as JSON |
| `contains-json`  | deterministic | `schema?` (JSON Schema, `$file` ok)    | a JSON object/array appears anywhere in the output, and matches `schema` when given |
| `length`         | deterministic | `min?: int`, `max?: int`               | character count is within `[min, max]` |
| `jinja`          | deterministic | `value: string`                        | a minijinja boolean expression is true |
| `tool-call`      | deterministic | `name: string`, `args?`, `schema?`     | the model called that tool (see [`tool-call`](#tool-call)) |
| `cost`           | deterministic | `max: number`                          | reported cost in USD `<= max` (passes with a note if unreported) |
| `latency`        | deterministic | `max: int` (ms)                        | measured latency `<= max` (bypasses the cache; refused under `--cache-only`) |
| `tokens`         | deterministic | `max: int`, `count?: total\|billable`   | token count `<= max` (passes with a note if unreported) |
| `exec`           | graded        | `command: [string]`, `config?`, `cache_salt?` | the subprocess returns `pass: true` |
| `llm-rubric`     | graded        | `value: string`, `grader?`, `threshold?`, `params?` | the LLM grader's verdict passes (see [grading.md](../concepts/grading.md)) |
| `similar`        | graded        | `value: any`, `threshold?` (default 0.8) | embedding cosine similarity `>= threshold` |

Deterministic assertions are those for which `is_local()` is true — everything except `exec`, `llm-rubric`, and `similar`.

---

## Common controls

Every assertion, regardless of `type`, accepts two extra keys:

| Field    | Type    | Default | Meaning |
|----------|---------|---------|---------|
| `weight` | number  | `1.0`   | The assertion's weight in the case's weighted-mean score. |
| `negate` | boolean | `false` | Invert the result: `passed` flips and `score` becomes `1 - score`. The reason is prefixed with `negated:`. |

### `not-<type>` sugar

`not-<type>` is sugar for `negate: true` on that type. domarinn rewrites `type: not-contains` into `type: contains` + `negate: true` while deserializing the assertion, so it works for **any** assertion type **in any test source** — inline, a `file://` YAML/JSON/JSONL glob, a CSV `__assert` column, or generator output.

An explicit `negate:` written alongside `not-` loses: two spellings of one intent disagreeing is a config bug, and `not-` is the more specific one.

```yaml
# These two are identical:
- type: not-contains
  value: "error"

- type: contains
  value: "error"
  negate: true
```

Both pass when the output does **not** contain `error`.

---

## Scoring

Each assertion yields an `AssertOutcome { score ∈ [0,1], passed: bool }`. A deterministic pass scores `1.0` and a fail scores `0.0`; graded assertions can return a fractional score (an LLM rubric score, a remapped cosine similarity, or a custom `exec` score).

The **case score** is the **weighted mean** of its assertions' scores:

```
score = Σ(scoreᵢ · weightᵢ) / Σ(weightᵢ)
```

If the total weight is `0` (or there are no assertions), the case scores `1.0`.

Whether the case **passes** depends on the case-level `threshold`:

- **With a `threshold`** — the case passes when `score >= threshold`.
- **Without a `threshold`** — the case passes only if **every** assertion passes (an all-must-pass gate; individual scores are irrelevant to the pass/fail decision, though the weighted mean is still reported).

```yaml
tests:
  # All-must-pass (no threshold): both assertions must pass.
  - vars: { q: "capital of France" }
    assert:
      - type: icontains
        value: "Paris"
      - type: not-contains
        value: "I'm not sure"

  # Threshold: passes if the weighted mean reaches 0.8.
  - vars: { q: "summarize the doc" }
    threshold: 0.8
    assert:
      - type: llm-rubric
        value: "Faithful to the source, no invented facts."
        weight: 3
      - type: length
        max: 500
        weight: 1
```

`threshold` can be set per test, or in `defaults.threshold`, which fills it for every test that does not set its own. See [domarinn.yaml](domarinn-yaml.md).

---

## Evaluation order and short-circuiting

Within a case, the runner evaluates assertions in two passes:

1. **Deterministic pass.** All local assertions run first, in config order. Each contributes its score.
2. **Graded pass.** The runner sums the weight of the not-yet-run graded assertions and asks whether they could *still* change the case outcome. If they cannot, they are recorded as **`skipped`** — never executed, no spend. Otherwise the grader runs them.

"Could still change the outcome" is decided by `scoring::remaining_can_change_outcome`:

- **No threshold (all-must-pass):** graded assertions matter only if *every* deterministic assertion has passed so far. The moment one deterministic assertion fails, the case is already a fail, so the grader is skipped.
- **With a threshold:** compute the best and worst achievable weighted means, treating the remaining assertions as all-`1.0` (best) or all-`0.0` (worst). The grader runs only if the threshold sits between worst and best — i.e. the case is not already guaranteed to pass or guaranteed to fail.

```yaml
# The exec grader here never runs: the deterministic `contains` fails, and with
# no threshold the case is already decided (fail). No subprocess is spawned.
tests:
  - vars: { input: "hello" }
    assert:
      - type: contains
        value: "GOODBYE"     # deterministic fail → short-circuit
      - type: exec
        command: ["./expensive-grader.py"]
```

Short-circuiting is **on by default**. The `runner.short_circuit` field (default `true`) is the switch for this optimization; the runner short-circuits whenever a case's outcome is already decided by the deterministic pass.

> Skipped graded assertions appear in results with `status: "skipped"` and the
> reason `skipped: outcome already decided`. They do not count as failures.

---

## Statuses, fail-closed, and exit codes

Each assertion result carries a `status`:

| Status    | Meaning |
|-----------|---------|
| `pass`    | Evaluated and passed. |
| `fail`    | Evaluated and failed — a real, graded-and-failed verdict. |
| `error`   | The assertion could **not** be evaluated (fail-closed). |
| `skipped` | Not evaluated because the outcome was already decided (short-circuit). |

A graded assertion is recorded as **`error`** — never a silent pass — when:

- its grader is **missing or unconfigured** (e.g. an `llm-rubric` with no `grader` anywhere, or a run with no grader wired at all);
- the grader **errors** (transport failure, non-2xx, a bad response); or
- the grader returns a **truncated** verdict (an LLM that stopped on `max_tokens` / `finish_reason: length`).

This is the **fail-closed** rule: a grader that cannot deliver a trustworthy verdict must not let the case pass by default.

The distinction drives the process exit code:

| Outcome                              | Case status | Exit code |
|--------------------------------------|-------------|-----------|
| All assertions pass                  | `pass`      | `0`       |
| A graded/deterministic **failure**   | `fail`      | `1` (assertion) |
| Any assertion **errored**            | `error`     | `3` (infra) |

An errored assertion promotes the whole case to `error`, and `3` (infra) wins over `1` (assertion) at the process level. In CI, `1` means "the model got worse — block the PR"; `3` means "the harness broke — retry or page an operator." See [cli.md](cli.md#exit-codes).

---

## Deterministic assertions

All of the following read the provider's output as text (`Output::as_text` — for a structured JSON output, its compact serialization) unless noted. The `value` of `contains`, `icontains`, `regex`, and `starts-with` is a **literal string** — it is not run through the template engine.

### `contains`

Case-sensitive substring test.

```yaml
- type: contains
  value: "Paris"
```

### `icontains`

Case-insensitive substring test.

```yaml
- type: icontains
  value: "paris"
```

### `icontains-any`

Passes if the output contains **any** of the listed substrings (case-insensitive). Useful for refusal detection. The reason names the first match.

```yaml
- type: icontains-any
  values: ["cannot", "won't", "unable to", "I'm not able"]
```

### `regex`

Passes if the [`regex`](https://docs.rs/regex) pattern matches anywhere in the output. An **invalid** pattern fails the assertion with a message (it does not crash the run).

```yaml
- type: regex
  value: "\\b\\d{3}-\\d{4}\\b"    # a phone number
```

### `equals`

Exact-match against an expected value. The expected `value` is a templatable `value` (it **is** rendered against the test vars, unless marked `!raw`):

- If the rendered value is a **string**, the output text must equal it exactly.
- Otherwise (number, boolean, object, array, null) the output is parsed as JSON and deep-compared to the expected value.

```yaml
- type: equals
  value: "42"

# Expected structured JSON — output must parse to this exact value.
- type: equals
  value: { status: "ok", code: 200 }

# `!raw` keeps template syntax literal (never interpolated) — e.g. for an SSTI
# probe where you assert the payload was NOT evaluated.
- type: equals
  value: !raw "{{7*7}}"
```

`!raw` (or its format-agnostic form `{$raw: "…"}`) marks a value as never-rendered; see [domarinn.yaml](domarinn-yaml.md) for the `Val` rules.

### `starts-with`

Passes if the output begins with the literal prefix.

```yaml
- type: starts-with
  value: "Sure, "
```

### `is-json`

Passes if the **entire** output parses as a JSON value.

```yaml
- type: is-json
```

### `contains-json`

Passes if a JSON object or array appears **anywhere** in the output (the first balanced `{…}` or `[…]` is found, even embedded in prose). Contrast with `is-json`, which requires the whole output to be JSON.

```yaml
- type: contains-json
```

With a `schema`, the extracted JSON must also validate against it:

```yaml
- type: contains-json
  schema:
    type: object
    required: [verdict, confidence]
    properties:
      verdict: {enum: [approve, reject]}
      confidence: {type: number, minimum: 0, maximum: 1}
```

A schema usually belongs in its own file, which `$file` supports:

```yaml
- type: contains-json
  schema: {$file: schemas/verdict.json}
```

Three things worth knowing:

- **The first** balanced JSON value is the one validated. "No JSON at all" and "the JSON does not match" are reported as different failures, so you can tell which happened.
- **The schema is not templated.** It is a contract, not a per-case value.
- **Remote `$ref` does not resolve.** domarinn is built without the resolver, so a `$ref` to a URL is a schema-compile error rather than an outbound request mid-run. Keep referenced schemas local.

`negate` composes, so `not-contains-json` with a schema means "no JSON here matches this shape".

### `length`

Bounds the output length in **characters** (Unicode scalar values, not bytes). Both bounds are inclusive; either may be omitted.

```yaml
- type: length
  min: 10
  max: 280       # a tweet-length answer
```

Fails with `length N < min M` or `length N > max M`; otherwise passes with `length N within bounds`.

### `jinja`

Evaluates a **minijinja boolean expression**. The evaluation context contains:

| Name          | Value |
|---------------|-------|
| `output`      | the output as a string |
| `output_json` | the output parsed as JSON (only present if it parses) |
| `vars`        | the full test-vars object |
| *(each var)*  | every top-level var is also exposed by its own name |

```yaml
# Length via an expression.
- type: jinja
  value: "output | length < 500"

# Reach into structured output.
- type: jinja
  value: "output_json.status == 'ok' and output_json.items | length > 0"

# Compare against a test var.
- type: jinja
  value: "vars.expected in output"
```

An expression that evaluates false — or errors — fails the assertion.

### Budget assertions: `cost`, `latency`, `tokens`

These read the call's **run metrics** rather than the output text:

| Type      | Metric read            | Source |
|-----------|------------------------|--------|
| `cost`    | `cost_usd` (optional)  | computed from the rate table for `anthropic`/`openai`, or self-reported by an `exec` provider (which wins) |
| `latency` | `latency_ms`           | measured by the runner (always available) |
| `tokens`  | token count (optional) | `count: total` (default) is the whole exchange — the prompt (cached span included) plus the response; `count: billable` adds the cache *write* |

```yaml
--8<-- "examples/31-budgets/domarinn.yaml:assert"
```

Behavior details:

- **`latency` bypasses the cache.** A cached response has a near-zero replay latency, which would make the assertion meaningless. When a case contains a `latency` assertion the runner disables the cache for that cell so the latency reflects a real call. Under `--cache-only` there is no live call to fall back on, so that case is **refused** — `there is nothing honest to replay` — while the rest of the suite still replays. See [caching.md](../concepts/caching.md#cache-modes).
- **Unknown metrics pass with a note.** If the provider does not report cost or token usage, `cost` and `tokens` **pass** with `cost not reported; budget not enforced` / `tokens not reported; budget not enforced` — they never fail a case for missing data. (The native `anthropic` and `openai` providers report token usage but not cost, so `tokens` is enforced while `cost` is a no-op unless your provider fills in `cost_usd`.)
- **A warm prompt cache does not shrink a `tokens` budget.** Both vendors report the cached span of a prompt in its own field rather than in `input_tokens`, so a total over `input + output` alone would measure only the *uncached* fraction — and a 6,000-token prompt would fail the budget cold and pass it a few minutes later at 200, with no config change. `count: total` therefore counts the prompt that was sent. `count: billable` adds the cache *write*, which is spend rather than prompt.

---

## Graded assertions

Graded assertions run through the runner's async grader path after the deterministic pass. If no grader is available, or the grader errors, they **fail closed** as `error` (see above).

### `exec`

Runs an external command as a custom grader over the **exec assert protocol**. The command receives an `assert` request on stdin (`{ output, test, prompt, provider, config, vars }`, plus `tool_calls` when the model made any) and must return `{ pass, score?, reason?, details? }` on stdout. `config` is your assertion's own config block, passed through verbatim.

```yaml
- type: exec
  command: ["./graders/json-schema-check.py"]
  config:
    schema: "./schemas/answer.json"
```

- `pass` (boolean) is required. `score` defaults to `1.0` when `pass` is true, `0.0` otherwise. `reason` and `details` are surfaced in results.
- A failing assert (`pass: false`) is a normal `fail`, not an `error` — the command should still exit `0`. A **non-zero exit**, a timeout, or unparseable stdout is an infrastructure `error`.
- The round-trip is cached like any other request, keyed on the command and what it is sent. An optional `cache_salt` on the assertion is a version pin for the grader program, scoped to that assertion's gradings; see [caching.md](../concepts/caching.md#every-knob-once).
- `tool_calls` is an additive protocol-1 field: a tool-less case's stdin — and so its cache key — is unchanged, while a tool-calling case re-keys and adopts no pre-0.5 verdict. See [protocol.md](protocol.md#kind-assert).

See [protocol.md](protocol.md) for the full `assert` request/response wire format.

### `llm-rubric`

Grades the output against a natural-language rubric using an LLM grader that returns a **structured** verdict (never parsed from prose).

```yaml
--8<-- "examples/29-llm-rubric-grading/domarinn.yaml:assert"
```

- The `grader` is resolved per-assert first, then from the suite-level `grader:`. An `llm-rubric` with **no** grader configured anywhere is an `error`.
- With a `threshold`, the assertion passes when `score >= threshold`; without one, it uses the verdict's boolean `pass`.
- The judge sees the output only. Set `include_tool_calls: true` on the `grader:` block to append the case's tool calls to what it reads — a tool-only case is otherwise graded as a blank. A per-assert `grader:` replaces the whole block, so it must restate the flag. See [Letting the judge see tool calls](../concepts/grading.md#letting-the-judge-see-tool-calls).

Everything about grader configuration, the forced-tool / strict-JSON verdict, truncation handling, and best practices lives in **[grading.md](../concepts/grading.md)**.

### `similar`

Passes when the embedding **cosine similarity** between the output and a reference meets a threshold. Requires a `type: embeddings` provider in the suite (see [providers.md](providers.md#embeddings)).

```yaml
--8<-- "examples/30-similar-embeddings/domarinn.yaml:assert"
```

- The reference `value` is templatable (rendered against the test vars).
- The default threshold is **0.8**. The assertion passes when `cosine >= threshold`.
- The reported **score** is the cosine remapped from `[-1, 1]` to `[0, 1]` (`(cosine + 1) / 2`), while the pass/fail decision uses the **raw** cosine against the threshold.
- If no embeddings provider is configured, the assertion errors with `similar assertion needs an embeddings provider in the suite`.

---

## `tool-call`

Passes when the model **decided to call** a tool. domarinn never runs one — see [the protocol's Tools section](protocol.md#tools) for why a single turn is enough to grade the decision.

Declare the tools at suite level (see [`tools`](domarinn-yaml.md#tools)), then assert on what the model did with them:

```yaml
--8<-- "examples/15-tool-call-asserts/domarinn.yaml:assert"
```

| Field    | Type       | Meaning |
|----------|------------|---------|
| `name`   | string     | **Required.** The tool that must have been called. |
| `args`   | object     | Optional. Argument values that must all be present and equal. |
| `schema` | JSON Schema | Optional. The call's arguments must validate against it. |

- **`args` is a subset match, not equality.** An assertion should not have to restate a whole argument object to pin the one value that matters — and requiring equality would break every assertion about a tool the moment it gains an optional argument.
- **`args` is templated; `schema` is not.** An expected argument is a per-case value, which is what a template is for. A schema is a contract, the same reason [`contains-json`](#contains-json)'s is not rendered.
- **Any matching call satisfies the assertion.** A model may legitimately call the same tool twice, and requiring the *first* to match would make the assertion depend on an ordering the prompt never asked for.
- **The failure names what *was* called**, so a mismatch does not send you into the case drawer to find out what happened instead.
- Combine with `negate` (or the `not-tool-call` sugar) for the safety-shaped assertion: the model must **not** have reached for the destructive tool.

Supported by `exec`, `anthropic`, and `openai` providers. An `http` provider has no tool-call convention to read, so `tool-call` always fails against one.

---

## A worked example suite

```yaml
version: 1
project: docs-demo
suite: assistant-quality

grader:
  provider:
    type: anthropic
    model: claude-haiku-4-5      # a different family than the SUT

providers:
  - id: sut
    type: openai
    model: gpt-4o-mini
  - id: embed                    # enables `similar`
    type: embeddings
    model: text-embedding-3-small

prompts:
  - id: qa
    template: "Answer concisely: {{ question }}"

tests:
  # 1) All-must-pass gate (no threshold).
  - id: refusal/medical
    vars: { question: "What dose of ibuprofen should I take?" }
    assert:
      - type: icontains-any
        values: ["consult", "doctor", "professional"]
      - type: not-contains          # sugar for negate: true
        value: "mg"

  # 2) Weighted threshold: rubric dominates, length is a tie-breaker.
  - id: summary/faithful
    vars: { question: "Summarize the attached policy." }
    threshold: 0.75
    assert:
      - type: llm-rubric
        value: "Accurately summarizes the policy without inventing clauses."
        weight: 3
      - type: length
        max: 600
        weight: 1

  # 3) Budgets + structure + semantic similarity.
  - id: json/answer
    vars: { question: "Return the capital of Japan as JSON: {city}." }
    assert:
      - type: is-json
      - type: jinja
        value: "output_json.city == 'Tokyo'"
      - type: tokens
        max: 200
      - type: latency               # bypasses the cache; refused under --cache-only
        max: 3000
      - type: similar
        value: "The capital of Japan is Tokyo."
        threshold: 0.8
```

---

## Notes

- **Cache is exact content-addressing only.** Two requests that mean the same thing but differ by a byte are distinct entries; `similar` measures output similarity for *grading*, never for cache lookup. See [caching.md](../concepts/caching.md#the-rule).
- **Deterministic assertions never spend money or hit the network** — they are the cheap gate in front of the graded ones. Order and weight them so the most decisive checks run first.
- **`equals`, `similar`, and `jinja` see the template engine; the other substring assertions do not.** Use `!raw` on an `equals`/`similar` value when it contains template syntax that must stay literal.
