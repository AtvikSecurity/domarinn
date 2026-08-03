# LLM-rubric grading

The `llm-rubric` assertion grades a provider's output against a natural-language **rubric** using an LLM as the **grader** — an LLM judge, in the usual phrasing. Unlike ad-hoc "ask the model and read the answer" grading, domarinn's grader **never parses a verdict out of prose**: it forces the model to return a *structured* verdict and treats anything less as an error.

> Source of truth: `crates/domarinn-core/src/grader.rs` and the `Grader` /
> `AssertKind::LlmRubric` types in `config.rs`. This assertion is introduced in
> [assertions.md](../reference/assertions.md#llm-rubric).

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

With [`include_tool_calls: true`](#letting-the-judge-see-tool-calls) on the grader, a third section is appended:

```
RUBRIC:
<your rubric, rendered with the test vars>

ASSISTANT OUTPUT:
<the provider output>

TOOL CALLS (the tool calls the assistant made, in order, as JSON):
<a pretty-printed array of {"name", "arguments"} objects>
```

---

## Grader resolution

An `llm-rubric` assertion needs a grader. It is resolved in this order:

1. The assertion's **own** `grader:` block (a per-assert override), if present.
2. Otherwise the **suite-level** `grader:` block.
3. If **neither** exists, the assertion is an **error** (fail-closed): `llm-rubric assertion has no grader configured (set suite grader or per-assert grader)`.

An errored assertion promotes the case to `error` and drives exit code `3`, not `1`. It is never a silent pass. See [assertions.md](../reference/assertions.md#statuses-fail-closed-and-exit-codes).

---

## Grader configuration

The `grader:` block wraps a provider plus grading options:

| Field          | Type          | Default    | Meaning |
|----------------|---------------|------------|---------|
| `provider`     | provider spec | –          | The grader model. Only `anthropic` and `openai` are supported for grading. |
| `template`     | string        | built-in   | Optional `file://` override of the grading-prompt template, relative to the suite directory. It renders into the prompt the grader reads, and [the request is the key](caching.md#the-rule) — so editing it re-grades. (An `exec` assertion's *program* is named by `command` and pinned by `cache_salt`, for the opposite reason: it receives the question rather than being part of it.) |
| `verdict_mode` | string        | `forced`   | How the structured verdict is obtained: `forced` (default) or `auto` (rejected at load — not implemented). |
| `include_tool_calls` | bool    | `false`    | Append the case's tool calls to the judge's user message. Off leaves the message byte-identical to before, so existing verdicts keep hitting the cache. See [Letting the judge see tool calls](#letting-the-judge-see-tool-calls). |

The `provider` is a standard [`ProviderKind`](../reference/providers.md) — but only the `anthropic` and `openai` shapes are valid graders. Any other provider type errors with `grader provider type … is not supported for llm-rubric`.

> `verdict_mode` and `template` are part of the grader schema. The implemented
> grading path always uses the **forced** structured-verdict mechanism
> described below (a forced tool call on Anthropic, a strict `json_schema`
> response on OpenAI); `forced` is the default and the mode you should rely on.

### Suite-level grader with a per-assert override

A suite-level `grader:` block, straight from a shipped example — the grader is a
different model family than the system under test, `max_tokens` is raised well
above the default, and the credential is read only by the grader (see
[Provider-specific mechanics](#provider-specific-mechanics)):

```yaml
--8<-- "examples/29-llm-rubric-grading/domarinn.yaml:grader"
```

Wired into a suite that uses it as the default grader, plus a per-assert
override:

```yaml
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

## Letting the judge see tool calls

The judge reads prose. A case the model answered with a tool call has no prose, so `ASSISTANT OUTPUT` is empty and the rubric grades a blank. `include_tool_calls: true` on the `grader:` block shows the judge what the model actually *did*:

```yaml
grader:
  provider:
    type: anthropic
    model: claude-haiku-4-5
  include_tool_calls: true
```

The user message then carries the third section shown [above](#the-verdict-shape) — `TOOL CALLS (the tool calls the assistant made, in order, as JSON):` followed by a pretty-printed array of `{"name", "arguments"}` objects, in the order the model made them. The provider's call `id` is **not** included: it is a per-response nonce, and putting it in the prompt would make every request unique and every verdict a cache miss.

**The section is always there when the flag is on.** Zero calls render `[]` rather than dropping the heading, so a rubric can judge the *absence* of a call ("answers from the context without calling `search`") as readily as its presence. A rubric that only ever sees the section when something happened cannot ask that question.

**Flipping the flag re-grades.** [The request is the key](caching.md#the-rule), and the flag changes the request — so turning it on re-asks the judge for the affected cells, and turning it back off restores the old message byte-for-byte and replays the old verdicts. With the flag off nothing changes at all, so an existing store keeps hitting.

**A per-assert `grader:` replaces the whole block, flag included.** An override is not a patch. An assertion with its own `grader:` must restate `include_tool_calls: true` or that one assertion grades blind while the rest of the suite does not:

```yaml
grader:
  provider: { type: anthropic, model: claude-haiku-4-5 }
  include_tool_calls: true

tests:
  - vars: { question: "What is the weather in Oslo?" }
    assert:
      # Uses the suite grader — sees the tool calls.
      - type: llm-rubric
        value: "Looks the weather up rather than guessing."

      - type: llm-rubric
        value: "Never reaches for a destructive tool."
        grader:
          provider: { type: openai, model: gpt-4o }
          include_tool_calls: true    # restated: the override replaced the whole block
```

**A custom `template` must agree with the flag.** A grader `template` substitutes `{{rubric}}` and `{{output}}` literally — plain string replacement, not the template engine — and gains a third placeholder, `{{tool_calls}}`, on the same terms. The two are checked against each other, both directions:

- Flag **off** and the template contains `{{tool_calls}}` — an error. The placeholder would go to the judge verbatim, as a section nothing ever fills.
- Flag **on** and the template omits `{{tool_calls}}` — an error. The flag would silently do nothing.

The check runs **when the template is read, at grading time** — not at load. A suite holding the contradiction validates clean and then fails the first graded cell that reaches it, as a fail-closed `grader misconfigured: …` on that assertion: the case is promoted to `error` and the run exits `3`. (A missing `{{rubric}}` or `{{output}}` stays tolerated; a template is allowed to grade on less than everything.)

**Do not combine it with `runner.skip_on_empty_reason: [tool_use_only]`.** That setting does *not* keep tool-only cells away from the judge — it overrides their verdict *after* grading, so you pay the judge for exactly the cells this flag exists for and then discard the answer as a `skip`. Pick one: report them as skips, or show them to the grader and let the verdict stand. See [excluding them from the verdict](#excluding-them-from-the-verdict).

**0.4.x verdicts are never adopted for a flag-on grading.** [Cache migration](caching.md#upgrading-to-05) adopts pre-0.5 grader entries on a miss, but every one of them was produced by a judge that could not see tool calls. Adopting one would answer the new question with the old answer.

---

## Empty outputs and grading

An empty output is a **successful** provider call. Nothing raises, nothing retries, and no assertion says why — the case simply has no text to grade. Four unrelated causes produce it, and each wants a different fix, so domarinn classifies the case with an `empty_reason`:

| Reason | What happened | Usual fix |
|---|---|---|
| `refusal` | The model declined. | Model behaviour, not a harness fault. |
| `content_filter` | A provider-side safety filter removed the content. | Same. |
| `truncated` | Cut off by `max_tokens` before any text. | Raise `max_tokens`. |
| `tool_use_only` | The model called a tool and said nothing else. | [Show the judge the calls](#letting-the-judge-see-tool-calls). |
| `thinking_only` | It reasoned but never emitted a final message. | Capture reasoning, or raise `max_tokens`. |
| `no_content_blocks`, `empty_body`, `output_expr_empty`, `blank` | Protocol-shaped faults, or a genuinely blank answer. | Read the raw response. |

**The set is open.** Reasons come from vendor finish reasons and grow at model-release cadence, so an unrecognized value is stored verbatim rather than collapsed to a catch-all. Match on what you see, never on a closed list.

### What an empty output does to the verdict

- **Positive assertions fail naturally.** There is nothing to contain, match, or judge, so they score `0.0` for the ordinary reason.
- **Negated assertions fail too, rather than passing vacuously.** Absence of forbidden content is not evidence of compliance when nothing was produced. See [the rule and its exact reason string](../reference/assertions.md#a-negated-assertion-cannot-pass-on-an-empty-output). It is a **fail**, never an error — a judgement about the output, not a broken assertion.
- **A cell that reported tool calls is graded normally.** `tool_use_only` is an empty *text* output by a model that did act, and there is real evidence to judge: the reported calls. Both `not-tool-call` and a rubric with `include_tool_calls: true` see them.
- **A cell whose output is not actually blank is graded normally too.** `empty_reason` is a claim (an `exec` child's is honoured even beside real text), so the guard re-checks the output before treating the cell as empty: real content gets real verdicts.
- **Metric assertions are unaffected.** `cost`, `latency` and `tokens` never read the output, so an empty answer says nothing about whether they hold.

**So a run can honestly report `Result: 300 passed` and `Empty: 4 (refusal × 4)` at the same time** — that is the intended reading, not a contradiction. It happens when the empty cells' only assertions were metric bounds (or when `skip_on_empty_reason` took them out of the verdict): the cases passed the questions actually asked of them, and the `Empty` tally exists precisely so that reading is visible instead of silently flattering the pass rate.

### Excluding them from the verdict

`runner.skip_on_empty_reason` removes matching cells from the **verdict**: they are reported `skip` rather than `fail`, so they stop dragging the pass rate down.

```yaml
runner:
  skip_on_empty_reason: ["refusal", "content_filter"]
```

It is a **verdict override, not a grading skip**, and the difference is worth money:

- **The assertions still run.** Every rule above still applies to the cell — including the vacuous-negation guard, which fires during evaluation, before the skip decision is taken. Their result simply no longer decides the case.
- **A rubric grader is still called, and still billed.** Nothing about this setting keeps a graded assertion away from the judge. If the spend is what you are trying to avoid, this is not the lever — [the graded-pass short-circuit](../reference/assertions.md#evaluation-order-and-short-circuiting) is the mechanism that skips a judge call, and it decides on weight arithmetic, not on `empty_reason`.
- **A broken assertion still errors the case.** An assertion that could not be evaluated at all outranks the skip: the cell is reported `error`, not `skip`. A config error stays visible rather than being quietly excused by the reason.
- **The assert results are kept.** The stored case still carries every assertion's status, score and reason, so the drawer explains what a `skip` was judged on.

The list matches the **classified** reason each case reports — the value shown in results, which domarinn fills in as `blank` when the provider named none — so list what you see there, not what you expect the provider to send.

### Where the reasons show up

| Surface | What you get |
|---|---|
| `domarinn run` terminal summary | An `{n} empty` segment in the stats footer, beside the pass rate it qualifies. The **total only** — no per-reason breakdown. |
| `--ci-summary` and `--format md` | An `Empty` row in the metrics table, carrying the full per-reason breakdown — `4 (refusal × 3, truncated × 1)` — in `BTreeMap` order, so it is stable across runs. |
| `GET /runs/{id}` | `empty_counts`: reason → count for the run. |
| `GET /runs` | `empty_count` per row. |
| `GET /runs/{id}/cases?empty_reason=refusal` | Filters cases to one reason (also on the `list_cases` MCP tool). |
| Web UI | An `Empty` column in the case grid, an `empty: <reason>` chip on the case drawer, and an empty count under **Cases** on the run header. |

Counts are **omitted, never `0`** when there is nothing to report — absence also covers runs stored before the field existed, so it is rendered blank rather than as a zero that would claim the run had none.

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
- The API key comes from `api_key_env` (default `OPENAI_API_KEY`); the base URL defaults to `https://api.openai.com/v1`, so any OpenAI-compatible gateway works as a grader via `base_url`.

A suite-level grader in this shape, from a shipped example — any OpenAI-compatible endpoint can be the grader, a local Ollama included (see [example 33](../examples/models-grading-and-budgets.md#example-33--an-openai-shaped-grader)):

```yaml
--8<-- "examples/33-openai-grader-rubric/domarinn.yaml:grader"
```

**Truncation is a loud error.** If `choices[0].finish_reason` is `length`:

> verdict truncated (finish_reason=length); raise grader max_tokens

### Parameters pass through verbatim

Whatever you put in the grader provider's `params` is merged into the request body **verbatim** — `top_p`, `top_k`, `max_tokens`, and so on. **No temperature is forced** by domarinn. The only rejected params are the thinking-enabling ones on Anthropic, as above.

---

## Scoring an `llm-rubric`

Given the verdict `{ pass, score }`, the assertion's pass/fail is:

- **With a `threshold`** on the assertion — pass when `score >= threshold`.
- **Without a `threshold`** — pass on the verdict's boolean `pass`.

The reported assertion score is always the verdict's `score` (clamped to `[0, 1]`); the verdict's `reasoning` becomes the assertion's reason. The case's weighted-mean score and pass/fail then follow the normal [scoring rules](../reference/assertions.md#scoring).

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

`grader.timeout_ms` covers `exec` assertions as well as the HTTP graders — the ceiling belongs to grading, not to a transport.

Non-2xx responses, transport errors, missing `tool_use`/content, and truncated verdicts all surface as grader **errors** (fail-closed), recorded as `grader error: …`.

---

## What grading costs

Grader calls are priced from the same built-in rate table as the systems under test, and a `grader.provider` accepts the same [`pricing:` override](../reference/providers.md#pricing). The same applies to the embeddings provider behind `similar`, which spends two calls per assertion (the output and the reference).

The figure is reported **separately** from the run's cost:

| Where | Field | Meaning |
|---|---|---|
| Run summary | `cost_usd` | What the systems under test cost. |
| Run summary | `grader_cost_usd` | What grading them cost. |
| Each assertion | `cost_usd` | What that one verdict cost. |

They are not added together on purpose. `cost_usd` is what a `cost:` assertion budgets, and a grader's price must not move a budget gate on the model being judged. It also stays honest about the common case where the grader is the more expensive model: a merged number would bury that.

A grader call is cached like any other request, and its cost is recorded with it — so a fully-cached run still reports what its grading is worth rather than dropping to zero, re-priced at today's rate. Which calls were actually paid for this time is visible per assertion, via `cached`. `--no-grader-cache` re-asks the grader while still replaying provider responses; see [caching.md](caching.md#cache-modes).

An `exec` grader reports nothing: the child spends against whatever endpoint it chose, and the protocol gives it no way to say so. A zero would claim custom grading is free.

---

## Best practices

- **Use a different model family for the grader than the system under test.** A model tends to prefer its own outputs (self-preference bias). If your SUT is GPT, judge with Claude, and vice-versa.
- **Prefer binary or clearly-anchored rubrics.** A crisp yes/no question ("Does the answer cite at least one source?") is far more reliable than a vague "rate the quality." When you do use a `score`, anchor the endpoints in the rubric ("1.0 = every claim is supported; 0.0 = a fabricated claim appears").
- **Write discriminative rubrics.** A good rubric separates good outputs from bad ones — if every plausible answer would pass, the assertion isn't testing anything. State what a failure looks like.
- **Keep `max_tokens` comfortable.** Because `reasoning` comes first, the model spends tokens before emitting `pass`/`score`. If you see truncation errors, raise `max_tokens` rather than shrinking the rubric.
- **Let deterministic checks gate the grader.** Put cheap [deterministic assertions](../reference/assertions.md#deterministic-assertions) alongside the rubric so obvious failures short-circuit the paid grader call.

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
