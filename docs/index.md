# domarinn

A declarative LLM eval harness: one static Rust binary that is a CLI, an evaluation engine, and a self-hostable results server with an embedded web UI. It treats **your own system** — a command, an HTTP endpoint, or a model — as the thing under test.

```yaml
--8<-- "examples/01-hello-eval/domarinn.yaml"
```

```console
$ domarinn run examples/01-hello-eval
```

## Start here

/// tip | New to domarinn?

Read [How a run works](concepts/how-a-run-works.md) first — four ideas that explain most of the behaviour — then work down the [Examples](examples.md) ladder. Every example on that page is a real directory in this repository, executed end to end in CI.

///

- **[Getting started](getting-started.md)** — install, write and run your first suite offline, add a model and an LLM grader, view and share results.
- **[Examples](examples.md)** — 32 runnable suites, one capability each.
- **[Scenarios](scenarios/index.md)** — end-to-end walkthroughs for a real situation.
- **[Migrating from promptfoo](migrate-promptfoo.md)** — `domarinn import promptfoo`, and what changes.

## Guides

| Guide | What it covers |
| ----- | -------------- |
| [Suite configuration](configuration.md) | The complete `domarinn.yaml` reference: providers, prompts, tests, defaults, grader, runner, cache, composition, and the `!raw` escape hatch. |
| [Assertions](assertions.md) | Every assertion type, weights and thresholds, short-circuiting, and fail-closed semantics. |
| [Providers](providers.md) | `exec`, `http`, `anthropic`, `openai`, and `embeddings`, plus the exec protocol. |
| [Grading](grading.md) | The LLM-rubric grader: structured tool-use / JSON-schema verdicts, fail-closed, grader selection. |
| [Caching](caching.md) | Content-addressed caching and sharing it between teammates (disk / server / S3 / layered). |
| [Statistics](statistics.md) | `--repeat`, Wilson confidence intervals, McNemar significance, pass@k, and baselines. |
| [Troubleshooting](troubleshooting.md) | Symptom, cause, fix — including the several ways a gate can be green and check nothing. |

## Running and hosting

| Doc | What it covers |
| --- | -------------- |
| [CLI reference](cli.md) | Every command and flag, and the CI exit-code contract. |
| [CI integration](ci.md) | Gating pull requests, the reusable GitHub Action, PR comments, and a shared cache in CI. |
| [Server](server.md) | The results server, the web UI, and accounts: local logins, roles, API keys, admin, auth modes. |
| [Deploy](deploy.md) | Docker, docker-compose, Kubernetes, backups, and reverse-proxy notes. |
| [Exec protocol](protocol.md) | The JSON protocol for writing providers, assertions, and test generators in any language. |

## Why domarinn

- **Your system is the system under test.** An external command or HTTP endpoint is a first-class provider; prompts are optional.
- **Real Jinja** with a per-value `!raw` escape hatch, so literal template syntax in a test input is never interpolated.
- **Deterministic assertions run first** and short-circuit the expensive grader.
- **The grader is structured and fails closed** — no verdicts parsed out of prose, no silent passes.
- **Errors are not failures.** Separate status, separate tally, separate exit code, so "the harness broke" never reads as "the model got worse".
- **Statistics built in** — confidence intervals and paired significance, not bare pass rates.
- **Caching you can share** over a server URL or an S3 bucket, because the key contains nothing about your machine.
- **One binary** — CLI, engine, and server, with the web UI embedded.

The reasoning behind each of these is in [Why domarinn is built this way](concepts/why-domarinn.md).
