# Test a model API

**The problem.** You are choosing between models, or you have pinned one and need to know when a new snapshot of it changes behaviour. Vendor benchmarks answer neither question — they measure a general capability, and you care about *your* prompts, *your* cases, and the answer changing under you.

**The shape.** One provider block per model, the same prompts and cases across all of them, and a budget assertion so a run cannot get expensive without you noticing.

## 1. Declare the model

Both native clients take the same four things — a model id, an endpoint, the **name of** an environment variable holding the key, and a params object.

/// tab | Anthropic

```yaml
--8<-- "examples/27-anthropic-provider/domarinn.yaml:provider"
```

Calls `POST {base_url}/v1/messages`. `ANTHROPIC_BASE_URL` is what the vendor's own tooling honours, so putting a gateway in front needs no edit here.

///

/// tab | OpenAI

```yaml
--8<-- "examples/26-openai-provider/domarinn.yaml:provider"
```

Calls `POST {base_url}/chat/completions` — which is the lingua franca. OpenAI itself, vLLM, LiteLLM, OpenRouter, Together, and a local Ollama all accept it, so this one block covers most of the market. See [Local LLMs](local-llms.md) for the loopback case.

///

Two rules about secrets, and both are load-bearing:

- **`api_key_env` names the variable, never the key.** The value is read at call time and never enters the suite, the cache key, or a shared run — which is what lets two teammates with different keys share cache entries instead of each paying separately.
- **`${env:VAR:-default}` resolves at load time and *does* enter the cache key.** Use it for things that change the answer — endpoint, model, mode — and never for credentials.

A `label` is worth setting whenever `model` is interpolated. It is the string the UI and every report use to name the system under test, and a run against one model must not report itself as another.

## 2. Compare two models in one run

A run is a grid of `providers × prompts × tests`, so a second provider does not replace the first — it doubles the cells and gives you a column to read against.

```yaml
providers:
  - id: haiku
    label: "claude-haiku-4-5"
    type: anthropic
    model: "claude-haiku-4-5"
    api_key_env: ANTHROPIC_API_KEY
    params: { max_tokens: 512 }
  - id: mini
    label: "gpt-4o-mini"
    type: openai
    model: "gpt-4o-mini"
    api_key_env: OPENAI_API_KEY
    params: { max_tokens: 512, temperature: 0 }
```

The [matrix view](../reference/web-ui.md#matrix-view) pivots that run to one column per provider, which is the view that answers "better on what" rather than "better". Scope a single run to one of them with `--provider mini` when you only want to re-measure one side.

## 3. Pass parameters through, verbatim

`params` is sent to the API **as-is**. domarinn sets only `model` and the messages; it forces no temperature and invents no defaults, so anything the vendor accepts is available without waiting for a domarinn release.

/// tab | Anthropic

```yaml
params:
  max_tokens: 512
  temperature: 0
  top_p: 0.9
  stop_sequences: ["\n\nHuman:"]
```

The Messages API requires `max_tokens`, so domarinn fills in `4096` when your `params` omit it. Set it deliberately rather than inheriting that.

///

/// tab | OpenAI

```yaml
params:
  max_tokens: 256
  temperature: 0
  seed: 7
  response_format: { type: json_object }
```

`temperature: 0` is not a determinism guarantee, but it removes the largest source of run-to-run noise.

///

Params are part of the cache key, so changing one re-asks rather than replaying — which is the point. See [Providers](../reference/providers.md) for the exact field list per type.

## 4. Make cost a fact, not a hope

domarinn ships a rate table for the models it knows and prices every call from it, so `cost_usd`, the run-level total, and a `cost:` budget all mean something with no configuration. A model it does **not** know — a preview snapshot, a negotiated rate, a gateway that rebills — prices at nothing, and then a `cost:` assertion passes as *"cost not reported; budget not enforced"*. Green, enforcing nothing.

State the price whenever you are not on public list rates:

```yaml
--8<-- "examples/27-anthropic-provider/domarinn.yaml:pricing"
```

Pricing is merged field-wise over the built-in rates and is deliberately **not** part of the cache key: `cost_usd` is recomputed on every cache hit from the stored token counts and the current rate sheet, so correcting a price re-prices your history instead of discarding it.

Then bound the run. [Example 31](../examples/models-grading-and-budgets.md#example-31--budgets) is the whole subject — cost, token and latency budgets, and the ways each of them can quietly enforce nothing:

```yaml
assert:
  - type: cost
    max: 0.05
  - type: tokens
    max: 2000
```

See [Pricing](../reference/providers.md#pricing) for what is priced (graders included, and reported separately) and [budget assertions](../reference/assertions.md#budget-assertions-cost-latency-tokens) for their exact semantics.

## 5. Grade what a substring cannot

Deterministic assertions get you further than people expect, and they should always run first — they are free and they short-circuit the expensive ones. When the property you care about genuinely needs judgment, add an `llm-rubric` and pin the judge separately from the system under test:

```yaml
grader:
  provider:
    type: anthropic
    model: "claude-haiku-4-5"
    api_key_env: ANTHROPIC_API_KEY
    params: { max_tokens: 4096 }
  verdict_mode: forced
```

A model grading its own output is not an independent measurement. The mechanics — verdict shape, fail-closed truncation, per-assert grader overrides, what grading costs — are in [Grading](../concepts/grading.md), and writing a rubric that measures one thing is [Guide 07](grade-against-policy.md).

If what you need to measure is a **tool decision** rather than prose, that is its own shape: declare `tools:` and assert on the `tool_calls` the model reports back. Over the native Anthropic API that is [example 35](../examples/models-grading-and-budgets.md#example-35--anthropic-tools-natively); over your own program it is [example 15](../examples/your-own-system.md#example-15--tool-call-assertions). Declaring a tool never makes domarinn run one.

## 6. Share the run, then keep the history

A local run prints and exits. `--share` uploads it to a results server, and that is when a suite turns into a trend:

```console
$ export DOMARINN_SERVER_URL=https://domarinn.example.com
$ export DOMARINN_TOKEN=<write-scoped token>
$ domarinn run eval/models.yaml --share
```

![The runs list after sharing: runs grouped by suite, with pass-rate trends](../assets/screenshots/runs-light.png#only-light)
![The runs list after sharing: runs grouped by suite, with pass-rate trends](../assets/screenshots/runs-dark.png#only-dark)

Shared runs land grouped by suite with a pass-rate sparkline per group, and every row carries who ran it, the branch and commit it ran against, and the token and cost totals — which is what makes "did that model snapshot change anything" a question you can answer next month instead of re-running from scratch. Filters live in the URL, so a link is a saved view.

From there, `--against server:baseline` gates a run against a pinned reference, and the [compare view](../reference/web-ui.md#compare--mcnemar) tells you whether a difference is real or noise. See [Gate a PR in CI](gate-in-ci.md).

## See also

- [Example 26](../examples/models-grading-and-budgets.md#example-26--an-openai-compatible-endpoint) and [example 27](../examples/models-grading-and-budgets.md#example-27--anthropic-and-what-it-costs) — the two provider blocks above.
- [Providers](../reference/providers.md) — every field of every provider type.
- [Local LLMs with Ollama](local-llms.md) — the same suites, zero spend, no key.
- [Test your app via exec](evaluate-your-app.md) — when the thing you ship is not the model.
