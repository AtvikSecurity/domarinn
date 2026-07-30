# Why domarinn is built this way

Most eval harnesses assume the thing under test is a model behind a prompt. That is one useful shape, and it is not the shape most teams are actually in: what ships is an application — with its own rendering, its own client, its own guardrails — and evaluating the model underneath it measures something adjacent to the product.

domarinn inverts the assumption. Below is what follows from that, and the handful of other decisions worth defending.

## External systems are first class

An `exec` command or an `http` endpoint is a normal provider, on equal footing with a native model client — the thesis, what it costs, and why that cost is `cache_salt`, are laid out in [Providers & the exec boundary](exec-boundary.md).

## Deterministic assertions run first

A substring check costs nothing and a judge costs money, so the cheap ones run first and can [short-circuit](how-a-run-works.md#cheap-assertions-run-first-and-can-stop-the-expensive-ones) the expensive ones. A case whose `contains` already failed never spawns a subprocess or calls a model.

This is not only a cost argument. Ordering the assertions by cost also orders them by *specificity*, and a failure report that leads with "did not contain the order number" is more useful than one that leads with a paragraph of judge reasoning.

## The grader is structured and fails closed

domarinn never asks a model for prose and greps it. Verdicts arrive as a forced tool call or a JSON-schema response carrying `pass`, `score` and `reasoning`, and anything missing, malformed or truncated is an **error**.

The alternative is worse in a specific, hard-to-notice way. Under a parse-the-prose scheme, a judge that ran out of tokens mid-sentence scores `0` — indistinguishable from a genuine failure of the thing under test. You go and debug a prompt that was fine. Failing closed turns a silent wrong answer into a loud one.

## Errors are not failures

A failed assertion means the system under test got worse. An error means the call never produced a gradeable answer, so **you learned nothing**. They get separate statuses, separate tallies and separate exit codes (`1` and `3`, with `3` winning).

A gate that reports both as "red" trains people to look past it, and a gate people look past is worse than no gate — it costs the same and provides false assurance.

## Statistics, not bare pass rates

"17/20 passed" is a point estimate presented as a fact. domarinn reports Wilson confidence intervals, pass@k, and McNemar paired significance when diffing two runs, because the question is almost never "what is the rate" — it is "did this change make things worse", and that question needs an error bar to answer.

McNemar in particular is a *paired* test: both runs saw the same cases, and treating them as independent samples discards exactly the information that makes the comparison sharp.

## Cache keys are portable by construction

**Hash what is sent.** One rule for every outgoing call, provider and grader alike: the key is the request itself. Nothing about the local filesystem, the binary, or a credential enters it.

This is the decision that makes a shared cache actually work. A key that varies by machine cannot be reused by anyone else, which turns a team cache or an S3 backend into an expensive local disk while appearing to function. The cost is that domarinn cannot detect a rebuild on its own; `cache_salt` is the deliberate, explicit lever for that, rather than a heuristic that would be wrong in both directions. See [caching.md](caching.md#the-rule).

## Real Jinja, with an escape hatch

Templating that cannot express a loop stops being useful about a week in. But a template engine applied to *test inputs* is a hazard: a case designed to prove a system does not evaluate `{{7*7}}` will feed it `49` instead, pass, and prove nothing.

So: full minijinja, plus a per-value [`!raw`](../examples/templates-and-test-data.md#example-06--the-raw-escape-hatch) that is never rendered. Interpolation is also deliberately narrow — `${env:…}` reaches provider config and *never* test vars, so an untrusted fixture cannot smuggle a different endpoint or key name into a provider.

## One binary

The CLI, the engine, the results server and the web UI are one static executable. There is no service to stand up before you can run your first suite, and no version skew between a CLI and a server that were released separately. See [Architecture — one binary](architecture.md) for how the one binary plays those roles, and where its data actually lives.

## See also

- [How a run works](how-a-run-works.md) — the mechanics.
- [Architecture — one binary](architecture.md) — the crates, the data, the versioned boundaries.
- [Providers & the exec boundary](exec-boundary.md) — the exec-boundary decision, in full.
- [Migrating from promptfoo](../guides/migrate-promptfoo.md) — what changes, concretely.
- [Examples](../examples/index.md) — each decision above, as something you can run.
