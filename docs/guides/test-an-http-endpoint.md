# Test an HTTP endpoint

**The problem.** The assistant you need to measure is already behind an HTTP API — your own service, a colleague's, a vendor's black box. There is no SDK you want to depend on, no wrapper process you want to write, and the response is your own JSON shape rather than anyone's chat-completions envelope.

**The shape.** `type: http` posts a templated request and pulls the answer out of the response with one expression. No adapter code.

/// tip | Which provider type

Use `type: http` when the endpoint is **yours** and the shape is yours. Use [`type: openai`](test-a-model-api.md) when it speaks chat-completions — that gets you token counts, cost, and a stop reason, none of which an arbitrary HTTP service reports. Use [`type: exec`](evaluate-your-app.md) when you would rather reuse your application's own client code than restate its request shape in YAML.

///

## 1. Post the request your service expects

```yaml
--8<-- "examples/28-http-provider/domarinn.yaml:provider"
```

`url`, `headers` and `body` are all **minijinja templates**, rendered per case against that case's vars plus `prompt` (the rendered prompt, as a string). So the provider adapts to your request shape rather than the other way round — `{{ user_message }}` above is a test var, and `locale` comes from `defaults.vars`.

Note that a `${...}` placeholder your own backend uses is left alone. Only the `${env:...}` sigil is claimed, so a body carrying your service's own template syntax passes through untouched.

## 2. Select the answer with `output_expr`

Without `output_expr`, the **raw response body** is the output — almost never what you want to assert on, because your envelope's `request_id` and status wrapper are then part of every substring check. The expression sees four things:

```
response.status    the HTTP status, as an integer
response.text      the raw body string
response.json      the parsed body, or null if it did not parse
response.headers   the response headers, as an object
```

`response.json`, not `response` — the latter is the envelope. Reach through it for the field that is actually the answer:

```yaml
output_expr: "response.json.result.reply"
```

Whatever the expression returns becomes the output **as-is**. A string stays text; anything else — a number, an object, an array — becomes structured output, which every text assertion still reads (stringified) and which `is-json` / `contains-json` can inspect directly. [Example 36](../examples/models-grading-and-budgets.md#example-36--output_expr-sliced-two-ways) points two providers at the same backend to show both halves of that:

```yaml
--8<-- "examples/36-http-output-expr/domarinn.yaml:provider"
```

Its second provider is the same URL and body with `output_expr: "response.json.result.confidence"` — a bare number — and a case scoped to it with `only_providers` asserts on that instead of the prose. Two slices of one response, graded independently.

`output_expr` only ever sees a **successful** response: a non-2xx status is a transport error before the expression runs, the same as any other provider's failure. That means the case reads as `error`, not as a failed assertion — see [how a run works](../concepts/how-a-run-works.md) for why that distinction is worth keeping. The full field list, and how each part lands in the cache key, is in [Providers → `http`](../reference/providers.md#http).

## 3. Authenticate without committing a secret

Credentials come from the environment at call time, exactly as with the model providers:

```yaml
headers:
  content-type: "application/json"
  authorization: "Bearer {{ env.SUPPORT_API_TOKEN }}"
```

The two `env` syntaxes are **not** interchangeable here, and picking the wrong one is the mistake this provider invites:

| Syntax | Resolves | In the cache key? | Use it for |
| ------ | -------- | ----------------- | ---------- |
| `${env:VAR}` | once, at **load** time | yes, as the substituted **value** | anything that changes the answer — the endpoint, a model selector, a mode |
| `{{ env.VAR }}` | per request, at **render** time | as a literal `${env:NAME}` placeholder, never the value | **credentials** |

Keying a token's value would give every API key its own private cache, so a team of five would pay five times for the same answers. Keying the *name* means they share. domarinn cannot tell a token from a model selector, so a provider whose url, headers or body reference `{{ env.X }}` **warns at startup naming the variable** — read that warning rather than silencing it. See [which `env` syntax](../concepts/caching.md#which-env-syntax).

Headers are in the key as a **digest** rather than as values, which is what makes both of those true at once: two providers differing only in an `X-Model` header do not share entries, while an `Authorization` holding two teammates' tokens still does.

/// danger | The resolved URL is stored in the run

`${env:…}` substitution happens at load time, so a run's `config_snapshot` carries the **resolved** `url` — and a shared run hands that to the results server verbatim, where the compare view renders it. An internal hostname put into a suite this way is an internal hostname in your results database. Keep private endpoints in per-developer config your VCS ignores, and see [Local LLMs](local-llms.md#5-wire-your-own-endpoint-per-developer) for that pattern.

///

## 4. Smoke the endpoint after a deploy

The same provider type covers a different job: not "is the assistant good" but "is the thing we just shipped answering at all". [Example 32](../examples/models-grading-and-budgets.md#example-32--a-live-endpoint) is that suite, and its whole design is that **every** `${env:VAR}` in it has no default:

```yaml
providers:
  - id: live
    type: openai                     # or http, for your own shape
    model: "${env:DOMARINN_SMOKE_MODEL}"
    base_url: "${env:DOMARINN_SMOKE_BASE_URL}"
    api_key_env: DOMARINN_SMOKE_API_KEY
```

An unset `${env:VAR}` without a default fails **at load time**, with the variable named. A smoke suite that silently fell back to a default would be the worst possible failure mode — green against staging while production is down — so it refuses to start instead.

Two things make this usable in a deploy pipeline:

- **A handful of cases, not a suite.** It runs on every deploy, so it must finish in seconds and cost approximately nothing.
- **`--no-cache`.** A smoke test asking whether the endpoint is up must not be answerable from a cache. This is the one place that flag belongs.

```console
$ DOMARINN_SMOKE_BASE_URL=https://api.internal.example/v1 \
  DOMARINN_SMOKE_MODEL=our-assistant-v3 \
  domarinn run eval/smoke.yaml --no-cache
```

Exit `0` means the endpoint answered and the answers held. Exit `3` — an **error**, not a failure — means you learned nothing, which is the signal a deploy gate should roll back on. The exit-code contract is in [the CLI reference](../reference/cli.md#exit-codes).

**No `--share` here, deliberately.** `run --share` is [fail-closed](../reference/cli.md#sharing-a-run): a run that cannot reach its results server exits `3` too. On a deploy gate that reads `3` as "roll back", that makes the results server a dependency of every deployment — an unreachable one rolls back a release whose endpoint was answering perfectly. Keep the gate reading the endpoint and nothing else. If you want the history too, upload it afterwards as its own step, where [`share`](../reference/cli.md#domarinn-share-run---strict) is best-effort by default and a failed upload cannot touch the deploy's verdict:

```console
$ DOMARINN_SERVER_URL=https://domarinn.example.com \
  DOMARINN_TOKEN=<write-scoped token> \
  domarinn share latest
```

## See also

- [Example 28](../examples/models-grading-and-budgets.md#example-28--a-service-you-already-run) and [example 36](../examples/models-grading-and-budgets.md#example-36--output_expr-sliced-two-ways) — the two provider blocks above.
- [Providers → `http`](../reference/providers.md#http) — every field, and exactly what is keyed.
- [Test your app via exec](evaluate-your-app.md) — the same goal, reusing your application's own code path.
- [Gate a PR in CI](gate-in-ci.md) — turning any of this into a merge gate.
