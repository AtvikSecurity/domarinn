# domarinn

A declarative LLM eval harness: one static Rust binary that is a CLI, an evaluation engine, and a self-hostable results server with an embedded web UI. It treats **your own system** — a command, an HTTP endpoint, or a model — as the thing under test.

```yaml
--8<-- "examples/01-hello-eval/domarinn.yaml"
```

```console
$ domarinn run examples/01-hello-eval
```

That suite is not a sketch. It is a real directory in this repository, it needs no API key and no model, and CI runs it end to end — as it does every other example on this site.

## Start here

One path, in order. Nothing in it costs money.

1. **[Install](start/install.md)** — a prebuilt static binary, `cargo`, source, or Docker.
2. **[Your first eval](start/first-eval.md)** — write and run the suite above offline, then add a real model and an LLM grader, and share the result.
3. **[How a run works](concepts/how-a-run-works.md)** — four ideas that explain most of the behaviour. Read it once and the rest of these docs stop being surprising.

Everything below is the map you come back to afterwards.

## Concepts

How domarinn thinks. Reach for these when a behaviour surprises you.

| Page | What it covers |
| ---- | -------------- |
| [How a run works](concepts/how-a-run-works.md) | The grid, short-circuiting, the fail-closed grader, and the cache key. |
| [Architecture — one binary](concepts/architecture.md) | The crates, the run document, and the versioned boundaries between them. |
| [Providers & the exec boundary](concepts/exec-boundary.md) | What a provider is, and what crosses the process boundary into yours. |
| [Grading](concepts/grading.md) | The `llm-rubric` grader: structured verdicts, fail-closed, and writing a rubric. |
| [The one-rule cache](concepts/caching.md) | The one key rule, every knob, salts, backends, and sharing a cache. |
| [Statistics](concepts/statistics.md) | `--repeat`, Wilson intervals, McNemar significance, pass@k, and baselines. |
| [Why domarinn is built this way](concepts/why-domarinn.md) | The reasoning behind each of the decisions above. |

## Guides

End-to-end walkthroughs for a real situation, each assembled from suites that run in CI. Six of the twelve, to give the shape; the [Guides](guides/index.md) page lists them all with what each one costs to run.

| Guide | When you reach for it |
| ----- | --------------------- |
| [Test a model API](guides/test-a-model-api.md) | Compare two models, or catch a snapshot changing under you. |
| [Test your app via exec](guides/evaluate-your-app.md) | Test what actually ships, not the model underneath it. |
| [A zero-cost gate on every PR](guides/render-gate.md) | Catch template and formatting regressions with no key and no spend. |
| [Gate a PR in CI](guides/gate-in-ci.md) | Block a merge that makes things worse, without demanding perfection. |
| [Migrate from promptfoo](guides/migrate-promptfoo.md) | `domarinn import promptfoo`, and exactly what changes. |
| [Troubleshooting](guides/troubleshooting.md) | Symptom, cause, fix — including the ways a gate can be green and check nothing. |

## Reference

Every field, flag, route and status code, verified against the code that reads it.

| Page | What it covers |
| ---- | -------------- |
| [domarinn.yaml](reference/domarinn-yaml.md) | Every top-level key of the suite file, and the `!raw` escape hatch. |
| [Providers](reference/providers.md) | `exec`, `http`, `anthropic`, `openai`, and `embeddings` — behavior and pricing. |
| [Assertions](reference/assertions.md) | Every assertion type, weights and thresholds, short-circuiting, fail-closed semantics. |
| [CLI](reference/cli.md) | Every command and flag, and the CI exit-code contract. |
| [Server](reference/server.md) | Running the results server, plus accounts: logins, roles, API keys, auth modes. |
| [REST API](reference/rest-api.md) | Every route, its shape, and the run-ingest contract. |
| [MCP endpoint](reference/mcp.md) | Read-only eval history for an agent, in the same binary. |
| [Exec protocol](reference/protocol.md) | The JSON protocol for writing providers, assertions, and generators in any language. |
| [The web UI](reference/web-ui.md) | Every view of the embedded UI, with a screenshot of each. |

## Examples

A ladder of 39 complete suites, one capability each, grouped into six pages. Every one is a real directory under [`examples/`](https://github.com/AtvikSecurity/domarinn/tree/main/examples), transcluded into the page you read it on and executed by CI — so a page cannot document a suite that stopped working. The [Examples](examples/index.md) index maps each number to its page.

| Group | Covers |
| ----- | ------ |
| [First steps](examples/first-steps.md) | The smallest suite that runs, up through weights and thresholds. |
| [Templates & test data](examples/templates-and-test-data.md) | `!raw`, file vars, matrix sweeps, datasets, generators, multi-turn prompts. |
| [Your own system](examples/your-own-system.md) | The `exec` provider, a custom assertion, tool-call grading, the protocol in bash. |
| [Running & reporting](examples/running-and-reporting.md) | Filters, composition, exit codes, retries, runner tuning, output formats, import. |
| [Caching & statistics](examples/caching-and-statistics.md) | The cache-key rule, salts, repeat and confidence, baselines and diff. |
| [Models, grading & budgets](examples/models-grading-and-budgets.md) | Model providers, HTTP and `output_expr`, rubric grading, similarity, budgets. |

## Why domarinn

- **Your system is the system under test.** An external command or HTTP endpoint is a first-class provider; prompts are optional.
- **Real Jinja** with a per-value `!raw` escape hatch, so literal template syntax in a test input is never interpolated.
- **Deterministic assertions run first** and short-circuit the expensive grader.
- **The grader is structured and fails closed** — no verdicts parsed out of prose, no silent passes.
- **Errors are not failures.** Separate status, separate tally, separate exit code, so "the harness broke" never reads as "the model got worse".
- **Statistics built in** — confidence intervals and paired significance, not bare pass rates.
- **Caching you can share** over a server URL or an S3 bucket, because every request — provider, grader, embedding — is keyed on the request itself and nothing about your machine.
- **One binary** — CLI, engine, and server, with the web UI embedded.

The reasoning behind each of these is in [Why domarinn is built this way](concepts/why-domarinn.md).
