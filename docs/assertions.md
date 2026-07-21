# Assertions

An **assertion** grades one provider output. A test case carries a list of
assertions under `assert:`; each produces a score in `[0, 1]` and a pass/fail
flag, and the case's own verdict is derived from them (see
[Scoring](#scoring)).

Assertions come in two families:

- **Deterministic** assertions need no network. They run **first**, in config
  order, so a cheap local failure can **short-circuit** the expensive graded
  assertions and avoid spending money.
- **Graded** assertions need an external call (a subprocess, an LLM grader, or
  an embeddings API). They run **after** the deterministic ones, and only if
  they can still change the case outcome.

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
| `contains-json`  | deterministic | `schema?` (reserved)                   | a JSON object/array appears anywhere in the output |
| `length`         | deterministic | `min?: int`, `max?: int`               | character count is within `[min, max]` |
| `jinja`          | deterministic | `value: string`                        | a minijinja boolean expression is true |
| `cost`           | deterministic | `max: number`                          | reported cost in USD `<= max` (passes with a note if unreported) |
| `latency`        | deterministic | `max: int` (ms)                        | measured latency `<= max` (this assert bypasses the cache) |
| `tokens`         | deterministic | `max: int`                             | total tokens `<= max` (passes with a note if unreported) |
| `exec`           | graded        | `command: [string]`, `config?`         | the subprocess returns `pass: true` |
| `llm-rubric`     | graded        | `value: string`, `grader?`, `threshold?`, `params?` | the LLM grader's verdict passes (see [grading.md](./grading.md)) |
| `similar`        | graded        | `value: any`, `threshold?` (default 0.8) | embedding cosine similarity `>= threshold` |

Deterministic assertions are those for which `is_local()` is true — everything
except `exec`, `llm-rubric`, and `similar`.

---

## Common controls

Every assertion, regardless of `type`, accepts two extra keys:

| Field    | Type    | Default | Meaning |
|----------|---------|---------|---------|
| `weight` | number  | `1.0`   | The assertion's weight in the case's weighted-mean score. |
| `negate` | boolean | `false` | Invert the result: `passed` flips and `score` becomes `1 - score`. The reason is prefixed with `negated:`. |

### `not-<type>` sugar

`not-<type>` is sugar for `negate: true` on that type. The loader rewrites
`type: not-contains` into `type: contains` + `negate: true` before the config
is deserialized, so it works for **any** assertion type.

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

Each assertion yields an `AssertOutcome { score ∈ [0,1], passed: bool }`. A
deterministic pass scores `1.0` and a fail scores `0.0`; graded assertions can
return a fractional score (an LLM rubric score, a remapped cosine similarity, or
a custom `exec` score).

The **case score** is the **weighted mean** of its assertions' scores:

```
score = Σ(scoreᵢ · weightᵢ) / Σ(weightᵢ)
```

If the total weight is `0` (or there are no assertions), the case scores `1.0`.

Whether the case **passes** depends on the case-level `threshold`:

- **With a `threshold`** — the case passes when `score >= threshold`.
- **Without a `threshold`** — the case passes only if **every** assertion
  passes (an all-must-pass gate; individual scores are irrelevant to the
  pass/fail decision, though the weighted mean is still reported).

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

`threshold` can be set per test case, or in `defaults.threshold` to apply to
every case in the suite. See [configuration.md](./configuration.md).

---

## Evaluation order and short-circuiting

Within a case, the runner evaluates assertions in two passes:

1. **Deterministic pass.** All local assertions run first, in config order.
   Each contributes its score.
2. **Graded pass.** The runner sums the weight of the not-yet-run graded
   assertions and asks whether they could *still* change the case outcome. If
   they cannot, they are recorded as **`skipped`** — never executed, no spend.
   Otherwise the grader runs them.

"Could still change the outcome" is decided by
`scoring::remaining_can_change_outcome`:

- **No threshold (all-must-pass):** graded assertions matter only if *every*
  deterministic assertion has passed so far. The moment one deterministic
  assertion fails, the case is already a fail, so the grader is skipped.
- **With a threshold:** compute the best and worst achievable weighted means,
  treating the remaining assertions as all-`1.0` (best) or all-`0.0` (worst).
  The grader runs only if the threshold sits between worst and best — i.e. the
  case is not already guaranteed to pass or guaranteed to fail.

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

Short-circuiting is **on by default**. The `runner.short_circuit` field
(default `true`) is the switch for this optimization; the runner short-circuits
whenever a case's outcome is already decided by the deterministic pass.

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

- its grader is **missing or unconfigured** (e.g. an `llm-rubric` with no
  `grader` anywhere, or a run with no grader wired at all);
- the grader **errors** (transport failure, non-2xx, a bad response); or
- the grader returns a **truncated** verdict (an LLM that stopped on
  `max_tokens` / `finish_reason: length`).

This is the **fail-closed** rule: a grader that cannot deliver a trustworthy
verdict must not let the case pass by default.

The distinction drives the process exit code:

| Outcome                              | Case status | Exit code |
|--------------------------------------|-------------|-----------|
| All assertions pass                  | `pass`      | `0`       |
| A graded/deterministic **failure**   | `fail`      | `1` (assertion) |
| Any assertion **errored**            | `error`     | `3` (infra) |

An errored assertion promotes the whole case to `error`, and `3` (infra) wins
over `1` (assertion) at the process level. In CI, `1` means "the model got
worse — block the PR"; `3` means "the harness broke — retry or page an
operator." See [cli.md](./cli.md#exit-codes).

---

## Deterministic assertions

All of the following read the provider's output as text
(`Output::as_text` — for a structured JSON output, its compact serialization)
unless noted. The `value` of `contains`, `icontains`, `regex`, and
`starts-with` is a **literal string** — it is not run through the template
engine.

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

Passes if the output contains **any** of the listed substrings
(case-insensitive). Useful for refusal detection. The reason names the first
match.

```yaml
- type: icontains-any
  values: ["cannot", "won't", "unable to", "I'm not able"]
```

### `regex`

Passes if the [`regex`](https://docs.rs/regex) pattern matches anywhere in the
output. An **invalid** pattern fails the assertion with a message (it does not
crash the run).

```yaml
- type: regex
  value: "\\b\\d{3}-\\d{4}\\b"    # a phone number
```

### `equals`

Exact-match against an expected value. The expected `value` is a templatable
`value` (it **is** rendered against the test vars, unless marked `!raw`):

- If the rendered value is a **string**, the output text must equal it exactly.
- Otherwise (number, boolean, object, array, null) the output is parsed as JSON
  and deep-compared to the expected value.

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

`!raw` (or its format-agnostic form `{$raw: "…"}`) marks a value as
never-rendered; see [configuration.md](./configuration.md) for the `Val` rules.

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

Passes if a JSON object or array appears **anywhere** in the output (the first
balanced `{…}` or `[…]` is found, even embedded in prose). Contrast with
`is-json`, which requires the whole output to be JSON.

```yaml
- type: contains-json
```

> The `schema` field is **reserved**. Today `contains-json` only checks for the
> *presence* of a JSON value; schema validation is not yet implemented, so a
> `schema:` you provide is accepted but ignored.

### `length`

Bounds the output length in **characters** (Unicode scalar values, not bytes).
Both bounds are inclusive; either may be omitted.

```yaml
- type: length
  min: 10
  max: 280       # a tweet-length answer
```

Fails with `length N < min M` or `length N > max M`; otherwise passes with
`length N within bounds`.

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
| `cost`    | `cost_usd` (optional)  | reported by the provider (e.g. an `exec` provider's `cost_usd`) |
| `latency` | `latency_ms`           | measured by the runner (always available) |
| `tokens`  | total tokens (optional)| `usage.input_tokens + usage.output_tokens` |

```yaml
- type: latency
  max: 2000          # ms
- type: cost
  max: 0.01          # USD
- type: tokens
  max: 1500
```

Behavior details:

- **`latency` bypasses the cache.** A cached response has a near-zero replay
  latency, which would make the assertion meaningless. When a case contains a
  `latency` assertion the runner disables the cache for that cell so the
  latency reflects a real call.
- **Unknown metrics pass with a note.** If the provider does not report cost
  or token usage, `cost` and `tokens` **pass** with `cost not reported; budget
  not enforced` / `tokens not reported; budget not enforced` — they never fail
  a case for missing data. (The native `anthropic` and `openai` providers
  report token usage but not cost, so `tokens` is enforced while `cost` is a
  no-op unless your provider fills in `cost_usd`.)

---

## Graded assertions

Graded assertions run through the runner's async grader path after the
deterministic pass. If no grader is available, or the grader errors, they
**fail closed** as `error` (see above).

### `exec`

Runs an external command as a custom grader over the **exec assert protocol**.
The command receives an `assert` request on stdin
(`{ output, test, prompt, provider, config }`) and must return
`{ pass, score?, reason?, details? }` on stdout. `config` is your assertion's
own config block, passed through verbatim.

```yaml
- type: exec
  command: ["./graders/json-schema-check.py"]
  config:
    schema: "./schemas/answer.json"
```

- `pass` (boolean) is required. `score` defaults to `1.0` when `pass` is true,
  `0.0` otherwise. `reason` and `details` are surfaced in results.
- A failing assert (`pass: false`) is a normal `fail`, not an `error` — the
  command should still exit `0`. A **non-zero exit**, a timeout, or unparseable
  stdout is an infrastructure `error`.

See [protocol.md](./protocol.md) for the full `assert` request/response wire
format.

### `llm-rubric`

Grades the output against a natural-language rubric using an LLM judge that
returns a **structured** verdict (never parsed from prose).

```yaml
- type: llm-rubric
  value: "The answer is polite and declines to give medical advice."
  threshold: 0.7            # optional: pass on score, not the boolean
```

- The `grader` is resolved per-assert first, then from the suite-level
  `grader:`. An `llm-rubric` with **no** grader configured anywhere is an
  `error`.
- With a `threshold`, the assertion passes when `score >= threshold`; without
  one, it uses the verdict's boolean `pass`.

Everything about grader configuration, the forced-tool / strict-JSON verdict,
truncation handling, and best practices lives in **[grading.md](./grading.md)**.

### `similar`

Passes when the embedding **cosine similarity** between the output and a
reference meets a threshold. Requires a `type: embeddings` provider in the
suite (see [providers.md](./providers.md#embeddings)).

```yaml
- type: similar
  value: "The mitochondria is the powerhouse of the cell."
  threshold: 0.85      # default is 0.8
```

- The reference `value` is templatable (rendered against the test vars).
- The default threshold is **0.8**. The assertion passes when
  `cosine >= threshold`.
- The reported **score** is the cosine remapped from `[-1, 1]` to `[0, 1]`
  (`(cosine + 1) / 2`), while the pass/fail decision uses the **raw** cosine
  against the threshold.
- If no embeddings provider is configured, the assertion errors with
  `similar assertion needs an embeddings provider in the suite`.

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
      - type: latency               # bypasses the cache for this case
        max: 3000
      - type: similar
        value: "The capital of Japan is Tokyo."
        threshold: 0.8
```

---

## Notes

- **Cache is exact content-addressing only.** There is no semantic /
  embedding-similarity dedupe on the cache: two prompts that mean the same
  thing but differ by a byte are distinct cache entries. `similar` measures
  output similarity for *grading*, not for cache lookup. See
  [caching.md](./caching.md).
- **Deterministic assertions never spend money or hit the network** — they are
  the cheap gate in front of the graded ones. Order and weight them so the most
  decisive checks run first.
- **`equals`, `similar`, and `jinja` see the template engine; the other
  substring assertions do not.** Use `!raw` on an `equals`/`similar` value when
  it contains template syntax that must stay literal.
</content>
</invoke>
