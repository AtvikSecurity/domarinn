# Models, grading & budgets

Ten suites about talking to a real model and judging what comes back. They cover the OpenAI-compatible and Anthropic providers (native tool calls included), a plain HTTP service you already run and the shapes `output_expr` can pull out of it, and a live endpoint of your own — then a structured LLM rubric judged by either vendor, embedding similarity for when many wordings are right, and the budgets that ask whether an answer was affordable, not just correct. Read these once you are ready to spend real tokens.

---

## Example 26 — An OpenAI-compatible endpoint

`type: openai` speaks the chat-completions API, which is the lingua franca: OpenAI itself, Ollama, vLLM, LiteLLM, OpenRouter and most gateways all accept it. Point `base_url` at whichever one you have.

```yaml
--8<-- "examples/26-openai-provider/domarinn.yaml"
```

/// danger | Two rules about secrets, both load-bearing

**`api_key_env` names the variable, never the key.** The value is read at call time and never enters the suite, the cache key, or a shared run.

**`${env:VAR:-default}` resolves at load time and *does* enter the cache key.** Use it for things that change the answer — endpoint, model, mode. Never for credentials: keying the value would give every API key its own private cache.

The counterpart is `{{ env.VAR }}`, which renders per request and is keyed as a literal `${env:NAME}` placeholder instead of its value. That is right for a credential and wrong for anything that changes the answer, because two values would share one cache entry and the second would replay the first's responses. domarinn warns when it sees `{{ env.X }}` in a URL, header or body, because it cannot tell a model selector from a token. It withholds that one hop only — a *case var* defined as `{{ env.SECRET }}` is resolved earlier and reaches the request in the clear.

///

Note the default: `https://api.openai.com/v1`. Setting `OPENAI_BASE_URL` — the same variable the vendor's own SDK honours — redirects the whole suite at a gateway or a local Ollama with no edit to the file. That is also exactly how this example is executed in CI, against a stub.

---

## Example 27 — Anthropic, and what it costs

Same shape as the OpenAI provider, plus the one thing that deserves its own example: telling domarinn what a call costs.

```yaml
--8<-- "examples/27-anthropic-provider/domarinn.yaml"
```

domarinn ships a rate sheet for the models it knows. A model it does **not** know prices at nothing — and a `cost:` assertion then *passes*, reporting "cost not reported; budget not enforced". Green, and enforcing nothing. Whenever you are on a negotiated rate, a preview model, or a gateway that rebills, state the price.

/// tip | Pricing is not in the cache key, on purpose

`cost_usd` is recomputed on every cache hit from the stored token counts and the current rate sheet. So correcting a price **re-prices your history** instead of discarding it — which is the behaviour you want the day you discover the rate was wrong.

///

Pricing is merged field-wise over the built-in rates, so you override only what differs.

---

## Example 28 — A service you already run

If your assistant is already behind an HTTP API, `type: http` is the shortest path from "it exists" to "it is measured" — no SDK, no wrapper process.

```yaml
--8<-- "examples/28-http-provider/domarinn.yaml"
```

`output_expr` is a minijinja expression over the response, so the provider adapts to *your* shape rather than the other way round. Four things are in scope:

| Expression | What it is |
| ---------- | ---------- |
| `response.status` | The HTTP status, as an integer. |
| `response.text` | The raw body string. |
| `response.json` | The parsed body, or `null` if it did not parse. |
| `response.headers` | The response headers, as an object. |

Note it is `response.json.result.reply`, not `response.result.reply` — `response` is the envelope, not the body. Without `output_expr` at all, the raw response *text* is the output, which is rarely what you want to assert on.

The cache key is the request this provider would send: the rendered `method`, `url` and `body`, plus a digest of the rendered `headers`. A `${…}` placeholder your own backend interprets is left untouched — only the `${env:…}` sigil is claimed. `output_expr` is in the key too: it never goes on the wire, but the entry stores the *projected* output, so editing it re-asks rather than replaying the old projection.

---

## Example 29 — LLM-rubric grading

`llm-rubric` asks a model whether an answer satisfies a rubric. It is the most expensive assertion and the easiest to misuse.

```yaml
--8<-- "examples/29-llm-rubric-grading/domarinn.yaml"
```

/// success | The verdict is structured, and fails closed

domarinn does not ask a judge for prose and grep it. It forces a tool call (or a JSON-schema response) carrying `pass`, `score` and `reasoning`. A missing, malformed or **truncated** verdict is an `error` — never a silent pass.

That matters more than it sounds. A judge that ran out of tokens mid-sentence would otherwise score `0` and read as a genuine failure of the thing under test, sending you to debug a prompt that was fine.

///

Three things about the grader block are deliberate. It names a **different model** from the one under test, because a model grading its own output is not an independent measurement. It raises `max_tokens` well above the default, because a thinking model can truncate a verdict at 1024 — and a generous ceiling costs nothing, since you are billed for tokens actually generated. And its `api_key_env` is read **only** by the grader: it does not inherit the provider's credential resolution, which fails asymmetrically and confusingly — completions succeed while every grade dies on 401, so the run looks like an infra fault rather than a credential one.

**Writing the rubric is the hard part.** Grade one axis; a rubric asking about correctness *and* tone *and* format returns one number that means none of them. Name the score-0 condition explicitly. And say what *not* to penalise — judges are eager, and without a "do not penalise verbosity or ordering" clause you are measuring the judge's taste.

---

## Example 30 — Similarity

`similar` embeds the output and a reference and compares them by cosine similarity. Reach for it when an answer is right in many wordings and you would otherwise be writing an `icontains-any` list that never ends.

```yaml
--8<-- "examples/30-similar-embeddings/domarinn.yaml"
```

/// warning | Two numbers, deliberately different

The **pass/fail decision** uses the raw cosine against `threshold`. The reported **score** is the cosine remapped from `[-1, 1]` to `[0, 1]`, i.e. `(cosine + 1) / 2`.

So `threshold: 0.85` means a cosine of 0.85, not a score of 0.85. And the default threshold of `0.8` is looser than most people expect — unrelated sentences in the same domain routinely clear 0.7.

///

It needs a `type: embeddings` provider in the suite; without one the assertion **errors** rather than passing. What gets cached is the two embeddings, not the similarity: each side is keyed on its own embedding request, which names the model. So changing the model re-embeds everything — as it must, since cosines are not comparable across models — while measuring the same output against a new reference re-embeds only the reference.

---

## Example 31 — Budgets

Three assertions answer "is this answer affordable" rather than "is it right".

```yaml
--8<-- "examples/31-budgets/domarinn.yaml"
```

/// danger | Each of these can pass without enforcing anything

- **`cost:`** passes when nothing priced the call — literally *"cost not reported; budget not enforced"*. That happens when the provider reports no usage, or the model is not in the rate sheet and the suite sets no `pricing:` block.
- **`tokens:`** needs the provider to report `usage`.
- **`latency:`** bypasses the cache entirely, because a replayed response has no honest latency. It measures a real call or nothing — which is why `--cache-only` refuses such a case outright instead of reaching the network.

A green cost budget is only evidence if you know the run priced itself.

///

`count: billable` is the **larger** number, not the smaller one: it is `total` *plus* the tokens paid to write a provider-side prompt cache, which are billed at a premium and are not part of the prompt the model answered. The second case shows the gap — the same call is 540 tokens by `total` and 2540 by `billable`. Budget `billable` when your provider writes a prompt cache, or the calls that cost the most are the ones you never see.

Keep latency bounds generous. A tight one is a flaky test on a loaded CI runner, and a flaky gate gets muted.

---

## Example 32 — A live endpoint

Everything above runs offline. This one is the opposite: it points at an OpenAI-compatible endpoint that only you have, and takes the endpoint, the model, and the name of the key variable entirely from the environment.

```yaml
--8<-- "examples/32-live-endpoint-smoke/domarinn.yaml"
```

Note that `api_key_env` names the *variable*, never the key. Nothing secret is committed, and nothing secret enters the cache key. Note also that these `${env:…}` interpolations carry no `:-default` — so with the variables unset, `domarinn validate` fails immediately and names the missing one, rather than a run failing later against a half-configured endpoint.

---

## Example 33 — An OpenAI-shaped judge

Example 29's grader was `type: anthropic`. A grader is a provider like any other, so it can just as easily be `type: openai` — which means any OpenAI-compatible endpoint can judge, a local Ollama included.

```yaml
--8<-- "examples/33-openai-grader-rubric/domarinn.yaml"
```

The system under test here is the offline echo provider, so the only thing that reaches a network is the judge — and the same one variable that redirects a *provider* in example 26 redirects this *grader* instead:

```
OPENAI_BASE_URL=http://localhost:11434/v1 OPENAI_MODEL=qwen3:4b domarinn run examples/33-openai-grader-rubric
```

/// tip | Grader identity is part of the cache key

The model and `base_url` resolve at load time like any other provider field. Judge a case with `gpt-4o-mini`, then again with `qwen3:4b`, and the second run asks fresh rather than replaying the first judge's verdict — two judges are two different questions, never one cache entry.

///

The rubric itself follows example 29's rules: one axis (policy fidelity, not tone or brevity), an explicit score-0 condition, and a line saying what *not* to penalise. The verdict's wire shape — a strict `json_schema` response this time, not a forced tool call — is [documented in full](../concepts/grading.md#openai-grader); the rubric-writing advice does not repeat here.

---

## Example 35 — Anthropic tools, natively

Example 15 declared tools for an `exec` provider — your own program decided whether to call one. The same `tools:` block and the same `tool-call` assertion work unchanged against `type: anthropic`: the suite declares tools once, and every API-shaped provider gets the same surface, in that vendor's native shape.

```yaml
--8<-- "examples/35-anthropic-tools/domarinn.yaml"
```

domarinn still never **executes** a tool, whichever transport carried the decision — see [Tools](../reference/protocol.md#tools).

---

## Example 36 — `output_expr`, sliced two ways

Example 28 pulled one nested string field out of a JSON body. `output_expr` is not limited to strings, or to one shape per suite — this one points two providers at the same backend and gives each a different expression.

```yaml
--8<-- "examples/36-http-output-expr/domarinn.yaml"
```

`only_providers` (from [example 16](running-and-reporting.md#example-16--tags-and-filters)) scopes each test to the provider it is actually testing — the two providers are not being compared, they are answering two different questions about the same response.

Whatever `output_expr` evaluates to becomes the output as-is: a string stays text, and anything else — a number here, but the same holds for an object or an array — becomes structured output. Every text assertion still reads it (stringified), and `is-json` / `contains-json` could inspect it directly. `output_expr` only ever sees a *successful* response: a non-2xx status is a transport error before the expression runs, same as any other provider's failure.
