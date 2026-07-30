# Local LLMs with Ollama

**The problem.** Every graded eval you write costs money to run and needs a secret to run at all — which means you cannot iterate on a rubric at your desk without paying per attempt, cannot put a graded suite in CI without provisioning a key, and cannot point any of it at customer text without sending that text to a vendor.

**The shape.** Serve an OpenAI-compatible endpoint on `localhost`, and redirect one variable. Nothing in a suite changes.

Three things make this work, and they are already in the suites you have:

- `type: openai` speaks **chat-completions**, which Ollama serves at `/v1`. So does vLLM, LiteLLM, and most gateways.
- Every endpoint in the shipped examples is `${env:OPENAI_BASE_URL:-…}` — the same variable OpenAI's own SDK honours.
- A grader is a provider like any other, so the same redirect moves **the grader** too.

## 1. Bring up a local endpoint

/// tab | Docker Compose

The repository's `docker-compose.yml` carries an `ollama` service behind its own profile, so a plain `docker compose up` never starts it:

```sh
docker compose --profile ollama up -d --wait ollama
docker compose --profile ollama exec ollama ollama pull qwen3:4b
```

The service publishes `11434` and keeps its models in a named volume, so a `down` does not cost you the download. To run on an NVIDIA GPU, uncomment the `deploy.resources.reservations.devices` block at the bottom of the service (it needs the NVIDIA Container Toolkit on the host).

///

/// tab | Bare Ollama

If Ollama is installed on the host, there is nothing to orchestrate:

```sh
ollama serve &          # already running as a service on most installs
ollama pull qwen3:4b
```

///

/// tab | A remote box

Models want a GPU and laptops usually do not have one. Forward the port instead of exposing the service:

```sh
ssh -L 11434:localhost:11434 your-llm-host
```

Everything below then works unchanged, because the endpoint is still `localhost` from domarinn's point of view — and the machine that holds the GPU never listens on a network.

///

Confirm it is up before blaming a suite:

```console
$ curl -s http://localhost:11434/v1/models | jq -r '.data[].id'
qwen3:4b
```

## 2. Redirect one variable

The provider in [example 26](../examples/models-grading-and-budgets.md#example-26--an-openai-compatible-endpoint) is written to be moved:

```yaml
--8<-- "examples/26-openai-provider/domarinn.yaml:provider"
```

```console
$ OPENAI_BASE_URL=http://localhost:11434/v1 \
  OPENAI_API_KEY=ollama \
  OPENAI_MODEL=qwen3:4b \
  domarinn run examples/26-openai-provider
```

`OPENAI_API_KEY` still has to be *set*: the client sends a bearer token unconditionally, and domarinn's [credential preflight](../reference/providers.md#credential-preflight) fails with exit `2` — naming the provider and the variable — before it makes a single call. Ollama ignores the value, so any non-empty string does. It is named by `api_key_env`, so whatever you put there never enters the suite, the cache key, or a shared run.

Note what `label` does in that fragment: it interpolates the same variable as `model`. A run against `qwen3:4b` reports itself as `qwen3:4b` on every page of the UI. A label hard-coded to `gpt-4o-mini` would quietly attribute local results to a model you never called, and eval results that misname the system under test are worse than no results.

/// warning | Cost is not reported, and that is not the same as zero

A local model is not in domarinn's rate table, so `cost_usd` is **unset** rather than `0`. Every [`cost:` budget](../reference/assertions.md#budget-assertions-cost-latency-tokens) in the suite then passes as "cost not reported; budget not enforced" — green, and enforcing nothing. That is fine while you are iterating locally; it is a trap if the same suite is your CI gate. Token budgets still work, because token counts are real.

///

## 3. Move the grader too

Example 33 is example 29's rubric with an OpenAI-shaped grader, which means any OpenAI-compatible endpoint can judge:

```yaml
--8<-- "examples/33-openai-grader-rubric/domarinn.yaml:grader"
```

```console
$ OPENAI_BASE_URL=http://localhost:11434/v1 OPENAI_API_KEY=ollama OPENAI_MODEL=qwen3:4b \
  domarinn run examples/33-openai-grader-rubric
```

The system under test in that suite is the offline echo provider, so the **only** thing reaching a network is the grader. That is the shape to reach for when you want to develop a rubric: the answers are fixed, the verdict is the variable, and each attempt is free.

Raise `max_tokens` and mean it. A thinking-capable model spends tokens before it emits the structured verdict, and a truncated verdict is a fail-closed **error**, not a silent pass. `4096` above is not generosity — you are billed for tokens generated, and locally you are billed nothing at all.

### A local grader is a different grader

![A locally graded run: one rubric verdict came back zero](../assets/screenshots/run-detail-local-light.png#only-light)
![A locally graded run: one rubric verdict came back zero](../assets/screenshots/run-detail-local-dark.png#only-dark)

That is a real run of [example 38](../examples/models-grading-and-budgets.md#example-38--every-key-once) graded by `qwen3:4b` on loopback, and it is worth showing precisely because it is **not** all green. Three cases, two passed, one failed at 0.75 — and the red square in that row's assert strip is the `llm-rubric`. The rubric asks one thing ("does the text name the product the customer is being answered about") and explicitly says not to penalise the response for reading as an instruction rather than an answer. The 4B grader answered:

> The text names "Aurora Notes" (a product), but the customer's question is about a jacket. The rubric requires the text to name the product the customer is being answered about (the jacket), not a different product.

A frontier grader scores that case 1.00. Neither verdict is a bug: a rubric is calibrated **against a grader**, and a smaller model reads instructions more literally and volunteers more opinions. So use a local grader for the loop you iterate in, and re-calibrate — or re-baseline — when you change the model that grades. Note also the `COST –` and the `100% cached` on this run: verdicts are cached per grader identity, which brings us to the caveat below.

## 4. What runs locally, and what does not

Not every example can be pointed at Ollama, and the reason is always the **wire shape**, never the model.

| Example | Local? | Why |
| ------- | ------ | --- |
| [26 — an OpenAI-compatible endpoint](../examples/models-grading-and-budgets.md#example-26--an-openai-compatible-endpoint) | yes | `type: openai`; set `OPENAI_BASE_URL` + `OPENAI_MODEL`. |
| [30 — similarity](../examples/models-grading-and-budgets.md#example-30--similarity) | yes | `type: embeddings` is also OpenAI-compatible. Add `OPENAI_EMBED_MODEL=nomic-embed-text` — a chat model does not serve `/v1/embeddings`. |
| [32 — a live endpoint](../examples/models-grading-and-budgets.md#example-32--a-live-endpoint) | yes | Endpoint, model and key are all `${env:…}` with no defaults. See [below](#5-wire-your-own-endpoint-per-developer). |
| [33 — an OpenAI-shaped grader](../examples/models-grading-and-budgets.md#example-33--an-openai-shaped-grader) | yes | Its grader is `type: openai`, so the same variable moves it. |
| [38 — every key, once](../examples/models-grading-and-budgets.md#example-38--every-key-once) | yes | Offline system under test, `type: openai` grader. This is how CI runs it. |
| [27 — Anthropic, and what it costs](../examples/models-grading-and-budgets.md#example-27--anthropic-and-what-it-costs) | no | `type: anthropic` speaks the **Messages** API. Ollama does not serve that shape. |
| [35 — Anthropic tools, natively](../examples/models-grading-and-budgets.md#example-35--anthropic-tools-natively) | no | Same wire shape, and the whole point of the example is the native tool block. |
| [29 — LLM-rubric grading](../examples/models-grading-and-budgets.md#example-29--llm-rubric-grading) | no | Its grader is `type: anthropic`. Use **33** instead — same rubric, OpenAI-shaped grader. |
| [28 — a service you already run](../examples/models-grading-and-budgets.md#example-28--a-service-you-already-run) | n/a | `type: http` posts *your* body shape to *your* endpoint; there is no model to relocate. See [Test an HTTP endpoint](test-an-http-endpoint.md). |
| [36 — `output_expr`, sliced two ways](../examples/models-grading-and-budgets.md#example-36--output_expr-sliced-two-ways) | n/a | Same: an arbitrary HTTP service, not a model API. |

To reach an Anthropic-shaped model from a local endpoint you need a gateway that translates (LiteLLM does), pointed at with `ANTHROPIC_BASE_URL`. Reaching for `type: openai` instead is usually less work.

### Which model

| Job | Start with | Why |
| --- | ---------- | --- |
| System under test | `qwen3:4b` | Small enough to be fast on a CPU, coherent enough that a suite's assertions mean something. |
| Grader | a larger `qwen3` | Verdicts are where model size shows. A 4B grader works and is what CI uses, but expect it to be stricter and noisier than a frontier one. |
| Embeddings | `nomic-embed-text` | A real embedding model. Chat models do not serve `/v1/embeddings` at all — the assertion errors rather than passing. |

A `similar` threshold does **not** transfer between embedding models: cosines are not comparable across them. Example 30's `threshold: 0.85` was chosen against a hosted model and scores around `0.76` under `nomic-embed-text` on the same paraphrase — a real fail, on a case that is genuinely a paraphrase. Re-choose the threshold for the model you run, and see [Troubleshooting](troubleshooting.md#a-similar-assertion-that-passes-at-any-threshold) for the other direction of that trap.

## 5. Wire your own endpoint, per developer

[Example 32](../examples/models-grading-and-budgets.md#example-32--a-live-endpoint) is deliberately unrunnable as shipped: every `${env:VAR}` in it has **no default**, so it names an endpoint only its reader has. That is the pattern for a post-deploy smoke suite, and the place to put those values is a per-developer file your VCS ignores. In this repository that is `.mise/config.local.toml`:

```toml
# .mise/config.local.toml — git-ignored; never commit an endpoint or a key.
[env]
DOMARINN_SMOKE_BASE_URL = "http://localhost:11434/v1"
DOMARINN_SMOKE_MODEL = "qwen3:4b"
DOMARINN_SMOKE_API_KEY = "ollama"
```

```console
$ domarinn run examples/32-live-endpoint-smoke
```

An unset `${env:VAR}` without a default fails **at load time**, with the variable named — so a missing value is a clear error before any call is made, not a mysterious 401 halfway through a run.

/// danger | The endpoint is stored in the run

A run's `config_snapshot` holds the **resolved** `base_url`, and the server returns it verbatim to the compare view. Point a suite at a private host, `--share` the run, and that hostname is now in your results database and in any screenshot of it. Keep private endpoints in ignored local config, and keep loopback in anything you publish.

///

## 6. The caveat that surprises everyone

`${env:VAR}` resolves at **load time**, which means the substituted value is part of the cache key. That is deliberate, and it is the behaviour you want:

- Switch `OPENAI_BASE_URL` from the hosted API to `localhost:11434` and the whole suite re-runs. Two different endpoints served two different answers; replaying one for the other would be a lie.
- Switch `OPENAI_MODEL` and the same thing happens, including for the grader — **grader identity is in the key**, so grading a case with `gpt-4o-mini` and then with `qwen3:4b` asks the second grader rather than replaying the first one's verdict.

The corollary: a warm local cache does not make your hosted runs cheaper, and it should not. Credentials are the exception — `api_key_env` names a variable whose value is read at call time and never keyed, so two teammates with different keys share cache entries instead of each paying separately. See [Caching → what is in the key](../concepts/caching.md#what-is-in-the-key) and [which `env` syntax](../concepts/caching.md#which-env-syntax).

## See also

- [Example 26](../examples/models-grading-and-budgets.md#example-26--an-openai-compatible-endpoint) and [example 33](../examples/models-grading-and-budgets.md#example-33--an-openai-shaped-grader) — the suites above.
- [Test a model API](test-a-model-api.md) — the same suites pointed at a vendor, with pricing and budgets.
- [Grading](../concepts/grading.md#openai-grader) — verdict mechanics, and what an OpenAI-shaped grader sends.
- [Providers → `openai`](../reference/providers.md#openai) — the full field list, including a gateway example.
