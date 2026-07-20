# measurellm documentation

measurellm is a declarative LLM prompt/eval harness: one static Rust binary that
is a CLI, an evaluation engine, and a self-hostable results server with an
embedded web UI. It treats your own system — a command, an HTTP endpoint, or a
model — as the thing under test.

## Start here

- **[Getting started](./getting-started.md)** — install, write and run your first
  suite offline, add a model and an LLM grader, view and share results.

## Guides

| Guide | What it covers |
|-------|----------------|
| [Configuration](./configuration.md) | The complete `measurellm.yaml` reference: providers, prompts, tests, defaults, grader, runner, cache, composition, and the `!raw` templating escape hatch. |
| [Assertions](./assertions.md) | Every assertion type, weights and thresholds, short-circuiting, and fail-closed semantics. |
| [Providers](./providers.md) | `exec`, `http`, `anthropic`, `openai`, and `embeddings` providers, plus the exec protocol. |
| [Grading](./grading.md) | The LLM-rubric grader: structured tool-use / json-schema verdicts, fail-closed, grader selection. |
| [Caching](./caching.md) | Content-addressed caching and sharing it between teammates (disk / server / S3 / layered). |
| [Statistics](./statistics.md) | `--repeat`, Wilson confidence intervals, McNemar significance, pass@k, and baselines. |
| [CLI reference](./cli.md) | Every command and flag, and the CI exit-code contract. |

## Running and hosting

| Doc | What it covers |
|-----|----------------|
| [Server](./server.md) | Running the results server, the web UI, and accounts: local logins, roles, API keys, admin, and auth modes. |
| [Deploy](./deploy.md) | Docker, docker-compose, Kubernetes, backups, and reverse-proxy notes. |
| [CI integration](./ci.md) | Gating pull requests, the reusable GitHub Action, PR comments, and shared cache in CI. |
| [Exec protocol](./protocol.md) | The JSON protocol for writing providers, assertions, and test generators in any language. |

## Why measurellm

- **Your system is the system under test.** An external command or HTTP endpoint
  is a first-class provider; prompts are optional.
- **Real Jinja** with a per-value `!raw` escape hatch, so literal template syntax
  in test inputs is never interpolated.
- **Deterministic assertions run first** and short-circuit the expensive grader.
- **The grader is structured and fails closed** — no verdicts parsed out of prose,
  no silent passes.
- **Statistics built in** — confidence intervals and paired significance, not bare
  pass rates.
- **Caching you can share** over a server URL or an S3 bucket.
- **One binary** — CLI, engine, and server, with the web UI embedded.
