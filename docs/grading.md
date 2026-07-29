# LLM-rubric grading

The `llm-rubric` assertion grades a provider's output against a natural-language **rubric** using an LLM as the judge. Unlike ad-hoc "ask the model and read the answer" grading, domarinn's grader **never parses a verdict out of prose**: it forces the model to return a *structured* verdict and treats anything less as an error.

> Source of truth: `crates/domarinn-core/src/grader.rs` and the `Grader` /
> `AssertKind::LlmRubric` types in `config.rs`. This assertion is introduced in
> [assertions.md](./reference/assertions.md#llm-rubric).

---

## The verdict shape

Every grader — regardless of provider — must produce the same three fields:

| Field       | Type    | Meaning |
|-------------|---------|---------|
| `reasoning` | string  | A brief justification. **Listed first** in the schema so the model reasons *before* it decides. |
| `pass`      | boolean | Did the output satisfy the rubric? |
| `score`     | number  | A `[0, 1]` graded score (clamped on ingest). |

The JSON Schema the grader enforces is exactly:

```json
{
  "type": "object",
  "additionalProperties": false,
  "properties": {
    "reasoning": { "type": "string" },
    "pass":      { "type": "boolean" },
    "score":     { "type": "number" }
  },
  "required": ["reasoning", "pass", "score"]
}
```

The built-in grading **system prompt** is:

> You are a strict evaluator. Grade the ASSISTANT OUTPUT against the RUBRIC.
> Return a boolean `pass`, a `score` in [0,1], and brief `reasoning`. Judge only
> what the rubric asks; do not reward effort.

The user message is assembled as:

```
RUBRIC:
<your rubric, rendered with the test vars>

ASSISTANT OUTPUT:
<the provider output>
```

---

## Grader resolution

An `llm-rubric` assertion needs a grader. It is resolved in this order:

1. The assertion's **own** `grader:` block (a per-assert override), if present.
2. Otherwise the **suite-level** `grader:` block.
3. If **neither** exists, the assertion is an **error** (fail-closed): `llm-rubric assertion has no grader configured (set suite grader or per-assert grader)`.

An errored assertion promotes the case to `error` and drives exit code `3`, not `1`. It is never a silent pass. See [assertions.md](./reference/assertions.md#statuses-fail-closed-and-exit-codes).

---

## Grader configuration

The `grader:` block wraps a provider plus grading options:

| Field          | Type          | Default    | Meaning |
|----------------|---------------|------------|---------|
| `provider`     | provider spec | –          | The judge model. Only `anthropic` and `openai` are supported for grading. |
| `template`     | string        | built-in   | Optional `file://` override of the grading-prompt template, relative to the suite directory. It renders into the prompt the judge reads, and [the request is the key](./caching.md#the-rule) — so editing it re-grades. (An `exec` assertion's *program* is named by `command` and pinned by `cache_salt`, for the opposite reason: it receives the question rather than being part of it.) |
| `verdict_mode` | string        | `forced`   | How the structured verdict is obtained: `forced` (default) or `auto` (rejected at load — not implemented). |

The `provider` is a standard [`ProviderKind`](./reference/providers.md) — but only the `anthropic` and `openai` shapes are valid graders. Any other provider type errors with `grader provider type … is not supported for llm-rubric`.

> `verdict_mode` and `template` are part of the grader schema. The implemented
> grading path always uses the **forced** structured-verdict mechanism
> described below (a forced tool call on Anthropic, a strict `json_schema`
> response on OpenAI); `forced` is the default and the mode you should rely on.

### Suite-level grader with a per-assert override

```yaml
version: 1

# Default judge for every llm-rubric in the suite.
grader:
  provider:
    type: anthropic
    model: claude-haiku-4-5
    params:
      max_tokens: 4096          # optional; defaults to 4096 when absent

providers:
  - id: sut
    type: openai
    model: gpt-4o-mini

prompts:
  - id: qa
    template: "{{ question }}"

tests:
  - vars: { question: "Explain TLS to a five-year-old." }
    assert:
      # Uses the suite-level grader above.
      - type: llm-rubric
        value: "Correct, uses a simple analogy, no jargon."

      # Overrides the grader just for this assertion.
      - type: llm-rubric
        value: "Strictly under three sentences."
        threshold: 1.0
        grader:
          provider:
            type: openai
            model: gpt-4o
```

---

## Provider-specific mechanics

### Anthropic grader

The grader calls the Messages API (`POST {base_url}/v1/messages`, `anthropic-version: 2023-06-01`) and **forces** a `submit_verdict` tool call:

- `tools` contains a single `submit_verdict` tool whose `input_schema` is the verdict schema above.
- `tool_choice` is `{ "type": "tool", "name": "submit_verdict" }`, so the model *must* answer through the tool.
- The verdict is read from the `tool_use` block's `input`. No prose is parsed.
- `max_tokens` defaults to **4096** when your `params` omit it.
- The API key comes from `api_key_env` (default `ANTHROPIC_API_KEY`); the base URL defaults to `https://api.anthropic.com`.

**Extended thinking is rejected.** Forced tool use is incompatible with extended thinking, so a grader whose `params` include `thinking` or `reasoning` is rejected up front:

> grader params must not enable extended thinking: forced tool use is rejected
> when thinking is on. Remove `thinking`/`reasoning`.

**Truncation is a loud error.** If the response stops on `stop_reason: max_tokens`, the verdict is considered truncated and the assertion errors:

> verdict truncated (stop_reason=max_tokens); raise grader max_tokens

Raise the grader's `max_tokens` (via `params`) and re-run.

### OpenAI grader

The grader calls chat completions (`POST {base_url}/chat/completions`) with a **strict** structured response:

- `response_format` is `{ "type": "json_schema", "json_schema": { "name": "verdict", "strict": true, "schema": <verdict schema> } }`.
- The verdict is parsed from `choices[0].message.content` (guaranteed to match the schema by strict mode).
- The API key comes from `api_key_env` (default `OPENAI_API_KEY`); the base URL defaults to `https://api.openai.com/v1`, so any OpenAI-compatible gateway works as a judge via `base_url`.

**Truncation is a loud error.** If `choices[0].finish_reason` is `length`:

> verdict truncated (finish_reason=length); raise grader max_tokens

### Parameters pass through verbatim

Whatever you put in the grader provider's `params` is merged into the request body **verbatim** — `top_p`, `top_k`, `max_tokens`, and so on. **No temperature is forced** by domarinn. The only rejected params are the thinking-enabling ones on Anthropic, as above.

---

## Scoring an `llm-rubric`

Given the verdict `{ pass, score }`, the assertion's pass/fail is:

- **With a `threshold`** on the assertion — pass when `score >= threshold`.
- **Without a `threshold`** — pass on the verdict's boolean `pass`.

The reported assertion score is always the verdict's `score` (clamped to `[0, 1]`); the verdict's `reasoning` becomes the assertion's reason. The case's weighted-mean score and pass/fail then follow the normal [scoring rules](./reference/assertions.md#scoring).

```yaml
# Binary: pass on the model's boolean judgment.
- type: llm-rubric
  value: "Does the answer correctly identify the bug?"

# Graded: require a high score, not just a yes.
- type: llm-rubric
  value: "Rate faithfulness to the source on the rubric below. 1.0 = every claim supported."
  threshold: 0.8
```

---

## Timeouts and defaults

| Setting                    | Value |
|----------------------------|-------|
| Grader call timeout        | 120 s, or `grader.timeout_ms` |
| Default grader `max_tokens`| 4096  |
| Default verdict mode       | `forced` |

`grader.timeout_ms` covers `exec` assertions as well as the HTTP judges — the ceiling belongs to grading, not to a transport.

Non-2xx responses, transport errors, missing `tool_use`/content, and truncated verdicts all surface as grader **errors** (fail-closed), recorded as `grader error: …`.

---

## What grading costs

Judge calls are priced from the same built-in rate table as the systems under test, and a `grader.provider` accepts the same [`pricing:` override](./reference/providers.md#pricing). The same applies to the embeddings provider behind `similar`, which spends two calls per assertion (the output and the reference).

The figure is reported **separately** from the run's cost:

| Where | Field | Meaning |
|---|---|---|
| Run summary | `cost_usd` | What the systems under test cost. |
| Run summary | `grader_cost_usd` | What grading them cost. |
| Each assertion | `cost_usd` | What that one verdict cost. |

They are not added together on purpose. `cost_usd` is what a `cost:` assertion budgets, and a judge's price must not move a budget gate on the model being judged. It also stays honest about the common case where the judge is the more expensive model: a merged number would bury that.

A judge call is cached like any other request, and its cost is recorded with it — so a fully-cached run still reports what its grading is worth rather than dropping to zero, re-priced at today's rate. Which calls were actually paid for this time is visible per assertion, via `cached`. `--no-grader-cache` re-asks the judge while still replaying provider responses; see [caching.md](./caching.md#cache-modes).

An `exec` grader reports nothing: the child spends against whatever endpoint it chose, and the protocol gives it no way to say so. A zero would claim custom grading is free.

---

## Best practices

- **Use a different model family for the grader than the system under test.** A model tends to prefer its own outputs (self-preference bias). If your SUT is GPT, judge with Claude, and vice-versa.
- **Prefer binary or clearly-anchored rubrics.** A crisp yes/no question ("Does the answer cite at least one source?") is far more reliable than a vague "rate the quality." When you do use a `score`, anchor the endpoints in the rubric ("1.0 = every claim is supported; 0.0 = a fabricated claim appears").
- **Write discriminative rubrics.** A good rubric separates good outputs from bad ones — if every plausible answer would pass, the assertion isn't testing anything. State what a failure looks like.
- **Keep `max_tokens` comfortable.** Because `reasoning` comes first, the model spends tokens before emitting `pass`/`score`. If you see truncation errors, raise `max_tokens` rather than shrinking the rubric.
- **Let deterministic checks gate the grader.** Put cheap [deterministic assertions](./reference/assertions.md#deterministic-assertions) alongside the rubric so obvious failures short-circuit the paid grader call.

---

## Full example

```yaml
version: 1
project: docs-demo
suite: rubric-grading

grader:
  provider:
    type: anthropic
    model: claude-haiku-4-5
    api_key_env: ANTHROPIC_API_KEY
    params:
      max_tokens: 2048          # pass-through; raises the truncation ceiling
  verdict_mode: forced          # default; shown for clarity

providers:
  - id: sut
    type: openai                # different family than the grader
    model: gpt-4o-mini

prompts:
  - id: support
    template: "Customer says: {{ message }}. Write a support reply."

tests:
  - id: tone/apology
    vars: { message: "My order arrived broken and I'm furious." }
    threshold: 0.75
    assert:
      # Cheap gate: short-circuits the grader if it fails.
      - type: not-icontains
        value: "not my problem"

      # Graded rubric with a scored threshold.
      - type: llm-rubric
        value: >
          Rate the reply. 1.0 = acknowledges the problem, apologizes, and offers
          a concrete next step (refund or replacement). 0.0 = dismissive or
          blames the customer.
        weight: 3

      # Per-assert grader override + strict binary gate.
      - type: llm-rubric
        value: "The reply contains no promise the company cannot keep (no fake dates)."
        grader:
          provider:
            type: openai
            model: gpt-4o
```
</content>
