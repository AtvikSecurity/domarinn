# Examples

A ladder of complete, runnable suites, each one demonstrating a single capability. Copy any of them, point the provider at your own system, and `domarinn run`.

/// info | Every suite on this page is executed in CI

The YAML below is not a transcription. It is pulled at build time from the real directories under [`examples/`](https://github.com/AtvikSecurity/domarinn/tree/main/examples), and every one of those directories is run end to end by `crates/domarinn-cli/tests/examples.rs` — which asserts the exit code, the pass/fail tally, and the exact case ids each suite produces. A page here cannot document a suite that no longer works, because the page and the test read the same bytes.

///

/// tip | The mental model

A run is a grid. domarinn takes the **providers** (the systems under test), the **prompts** (optional — a provider may build its own input), and the **tests** (the cases), and evaluates one **cell** per combination. Each cell calls its provider once and grades the answer against that case's **assertions**.

Deterministic assertions run first and can short-circuit the expensive ones, so a case that fails `contains` never pays for an LLM grader. A cell's status is `pass`, `fail`, `error` (the harness broke — never counted as an assertion failure), or `skip`.

///

| #   | Example | Demonstrates |
| --- | ------- | ------------ |
| 01  | [Hello, eval](#example-01--hello-eval) | The smallest suite that runs. No model, no key, no toolchain. |
| 02  | [Prompts and variables](#example-02--prompts-and-variables) | Prompt templates filled per case — and why a run is a grid. |
| 03  | [Deterministic assertions](#example-03--deterministic-assertions) | Every zero-cost assertion type, on one page. |
| 04  | [Structured output](#example-04--structured-output) | `is-json` versus `contains-json` with a schema. |
| 05  | [Weights and thresholds](#example-05--weights-and-thresholds) | How a case decides pass or fail, and how to give partial credit. |
| 06  | [The `!raw` escape hatch](#example-06--the-raw-escape-hatch) | Test input that must reach the system byte-for-byte. |
| 07  | [File-content vars](#example-07--file-content-vars) | Pull a var's value from a file beside the suite — parsed, raw, or sandboxed. |
| 08  | [Matrix sweeps](#example-08--matrix-sweeps) | Fan one case out over the cartesian product of its axes. |
| 09  | [Datasets from files](#example-09--datasets-from-files) | Cases in `file://` globs, owned and reviewed separately. |
| 10  | [A CSV dataset](#example-10--a-csv-dataset) | The format a non-engineer will hand you, read directly. |
| 11  | [Test generators](#example-11--test-generators) | Cases computed by a program, so coverage cannot drift. |
| 12  | [Render health](#example-12--render-health) | Grade an external system with zero-LLM assertions. |
| 13  | [Your own system](#example-13--your-own-system) | The `exec` provider: test what actually ships. |
| 14  | [A custom assertion](#example-14--a-custom-assertion) | A correctness rule only you can express. |
| 15  | [Tool-call assertions](#example-15--tool-call-assertions) | Grade the decision, not the prose. |
| 16  | [Tags and filters](#example-16--tags-and-filters) | Running part of a suite. |
| 17  | [Composition](#example-17--composition) | `extends`, `imports`, and how the merge actually works. |
| 18  | [A failing gate](#example-18--a-failing-gate) | What red looks like. Exits 1 on purpose. |
| 19  | [Errors and retries](#example-19--errors-and-retries) | Errors are not failures. Exits 3 on purpose. |
| 20  | [Runner tuning](#example-20--runner-tuning) | Concurrency, rate limits, timeouts. |
| 21  | [Caching](#example-21--caching) | Not paying twice for the same answer. |
| 22  | [Cache salts](#example-22--cache-salts) | Busting the cache at the right granularity. |
| 23  | [Repeat and confidence](#example-23--repeat-and-confidence) | A pass rate with an error bar. |
| 24  | [Baselines and diff](#example-24--baselines-and-diff) | Gate on regressions, not on an absolute score. |
| 25  | [Output formats](#example-25--output-formats) | One run feeding a human and a machine. |
| 26  | [An OpenAI-compatible endpoint](#example-26--an-openai-compatible-endpoint) | The lingua franca: OpenAI, Ollama, vLLM, LiteLLM, a gateway. |
| 27  | [Anthropic, and what it costs](#example-27--anthropic-and-what-it-costs) | `pricing`, and why a `cost:` budget can be green and enforce nothing. |
| 28  | [A service you already run](#example-28--a-service-you-already-run) | The `http` provider and `output_expr`. |
| 29  | [LLM-rubric grading](#example-29--llm-rubric-grading) | A structured, fail-closed judge — and how to write its rubric. |
| 30  | [Similarity](#example-30--similarity) | Cosine distance, for when many wordings are right. |
| 31  | [Budgets](#example-31--budgets) | Cost, tokens, latency — and how each can enforce nothing. |
| 32  | [A live endpoint](#example-32--a-live-endpoint) | Point a suite at your own OpenAI-compatible endpoint. |

---

## Example 01 — Hello, eval

The smallest thing that works. One provider, one case, one assertion.

The system under test is a shell one-liner that prints a fixed answer, so this suite needs no model, no API key, and not even Python — just a POSIX shell. Read it as three answers: `providers` is **what** is being tested, `tests` is **which** inputs it gets, and `assert` is **what must be true** of the answer.

```yaml
--8<-- "examples/01-hello-eval/domarinn.yaml"
```

An `exec` provider is any program that reads one JSON request on stdin and writes one JSON response on stdout. That is the entire contract — see the [exec protocol](protocol.md). Swap the `command` for your own program and this suite is already testing your system.

---

## Example 02 — Prompts and variables

A prompt is a template; a case's `vars` fill it in. domarinn renders the prompt once per case with real Jinja, then hands the result to the provider.

/// warning | A run is a grid, not a list

Every prompt is evaluated against every test. Two prompts and three cases is **six cells**, not three. That is the point — comparing two phrasings of the same instruction on identical inputs is what a prompt suite is *for* — but it means every assertion has to hold for both prompts. An assertion that only makes sense for one of them will fail half the grid.

///

```yaml
--8<-- "examples/02-prompts-and-vars/domarinn.yaml"
```

Two prompt shapes appear here. `template:` is a single string, which suits a provider that takes one blob of text. `messages:` is a chat transcript with roles, which suits a provider that talks to a chat API. A prompt sets exactly one of the two.

`defaults.vars` supplies values every case inherits, so a case states only what makes it different — and may override an inherited value, as `refund/other-product` does.

---

## Example 03 — Deterministic assertions

These run locally, cost nothing, and need no model. They also run **first**: a case whose `contains` fails never pays for an LLM grader. Reach for a rubric only for what these cannot express.

```yaml
--8<-- "examples/03-deterministic-asserts/domarinn.yaml"
```

/// note | `jinja` is minijinja, not Python

The `jinja` assertion evaluates a boolean expression with `output` and the case's vars in scope. It is Jinja2 semantics, so strings have no `.startswith()` method — use the `in` operator, or a filter like `output | length`. An expression that errors is recorded as an assertion **error**, not a silent failure.

///

---

## Example 04 — Structured output

When a system is asked for JSON there are two separate questions, and it pays to assert them separately: *is it parseable at all* (`is-json`), and *does it have the right shape* (`contains-json` with a `schema`).

The difference matters because a model that wraps its JSON in prose fails the first and passes the second. That is the most common failure mode of an under-instructed model, so the example asserts it explicitly rather than hiding it.

```yaml
--8<-- "examples/04-json-output/domarinn.yaml"
```

The schema is ordinary JSON Schema. `required` is what turns "it parsed" into "it has the fields we depend on" — without it, a model that returns `{}` passes.

---

## Example 05 — Weights and thresholds

Each assertion scores `1.0` or `0.0` (a graded assertion may score in between), and the case score is their weighted mean:

```
score = Σ(scoreᵢ · weightᵢ) / Σ(weightᵢ)
```

What that score *means* depends on whether the case sets a `threshold`:

- **no threshold** — all-must-pass. One failed assertion fails the case.
- **a threshold** — the case passes when `score >= threshold`.

```yaml
--8<-- "examples/05-weights-and-thresholds/domarinn.yaml"
```

Two of these cases pass *with a failing assertion inside*, which is the whole subject: `gate/partial-credit` scores `(1 + 1 + 0) / 3 = 0.667` and clears its `0.6` threshold, and `gate/weighted` scores `(1×3 + 0×1) / 4 = 0.75` and clears `0.7`. Weighting the amount three times the pleasantry is what makes a missing pleasantry cost a quarter of the score instead of half.

Use a threshold when an answer can be partly right and you want to say how right is right enough. Use the default when every assertion is a hard requirement.

Any assertion type can be negated by prefixing `not-`, as `gate/must-not-leak` shows. This works in every test source — inline YAML, a `file://` dataset, a CSV column, or a generator's output.

---

## Example 06 — The `!raw` escape hatch

Every var is rendered through Jinja. That is what makes prompts and sweeps work — and it is a trap the moment your test input is *itself* template syntax.

/// danger | This failure is silent

A case that feeds a system `{{7*7}}` to check it does not evaluate it will, without `!raw`, feed it `49` instead. The assertion still passes. The test now proves nothing, and nothing tells you.

///

```yaml
--8<-- "examples/06-raw-escape-hatch/domarinn.yaml"
```

Two spellings, identical after loading: the `!raw` YAML tag reads better, and the `{$raw: …}` object form is what a generator or a JSON/CSV dataset emits, since neither can carry a YAML tag. Note that `!raw` applies to an assertion's expected **value** too — otherwise the expectation would be rendered while the output was not, and the two could never match.

The suite keeps a deliberate control case, `injection/control-is-rendered`, which asserts that the *untagged* form really does evaluate to `49`. It is the evidence that the tag above is doing work.

---

## Example 07 — File-content vars

A var can take its value from a file next to the suite instead of being written inline — useful for large documents, golden fixtures, and adversarial inputs you would rather not paste into YAML.

Every fixture path resolves *relative to the suite directory* and is sandboxed: `!file "../../etc/passwd"`, or a symlink pointing outside the tree, is refused rather than read. The four cases below cover the whole surface — a plain text fixture, a `.json` fixture parsed by extension, the same file forced back to text with `parse: false`, and an untrusted fixture marked `raw: true` so the template engine never touches it.

```yaml
--8<-- "examples/07-file-vars/domarinn.yaml"
```

/// warning | `raw: true` is not optional for untrusted input

Every var goes through Jinja. A fixture containing `{{7*7}}` renders as `49` unless you mark it raw — which silently destroys the premise of any test whose whole point is that the payload stayed literal. See [Suite configuration](configuration.md).

///

---

## Example 08 — Matrix sweeps

One case fans out over the cartesian product of its axes, producing one concrete case per combination. Each axis value is merged into `vars`, where it wins over a base var of the same name.

The ids are deterministic — `greet[style=terse,temperature=0]` and friends — which is what lets [`domarinn diff`](statistics.md) line two runs up cell by cell. When the generated shape is unwieldy, `matrix_id` renders a friendlier one against the axis values.

```yaml
--8<-- "examples/08-matrix-sweeps/domarinn.yaml"
```

This expands to seven cells: a 2×2 sweep over `style` and `temperature`, plus a three-value `lang` sweep.

---

## Example 09 — Datasets from files

Inline `tests:` stop scaling somewhere around thirty cases. A `file://` glob keeps the suite readable and lets a dataset be owned, reviewed and diffed separately from the configuration that runs it.

```yaml
--8<-- "examples/09-dataset-glob/domarinn.yaml"
```

A dataset file is a bare sequence of cases — the same shape an inline entry has, with no wrapper key:

```yaml
--8<-- "examples/09-dataset-glob/cases/refunds.yaml"
```

Formats are chosen by extension: `.yaml`/`.yml`, `.json`, `.jsonl`/`.ndjson`, `.csv`, `.tsv`. Every path resolves relative to the suite directory and is sandboxed — `file://../../etc/passwd`, or a symlink pointing out of the tree, is refused rather than read. A glob that matches nothing is an error, not an empty suite.

---

## Example 10 — A CSV dataset

CSV is the format a non-engineer will hand you, so domarinn reads it directly. Four column names are reserved — `id`, `tags`, `cache_salt`, and `__assert` — and every other column becomes a var of that name.

```yaml
--8<-- "examples/10-dataset-csv/domarinn.yaml"
```

```csv
--8<-- "examples/10-dataset-csv/cases.csv"
```

`__assert` holds the case's assertions as JSON, which is what lets a spreadsheet express `not-icontains` without inventing a column convention. `.tsv` works identically with tabs. For anything more structured than this, reach for `.jsonl`.

---

## Example 11 — Test generators

A glob reads cases someone wrote. A **generator** computes them — so coverage tracks the thing being covered instead of drifting from it. If your prompts, tools or policies live in a registry, enumerate the registry and every new entry gets a test for free, on the day it is added.

```yaml
--8<-- "examples/11-test-generators/domarinn.yaml"
```

A generator is an exec-protocol program like any other: it reads one JSON request on stdin and writes `{"tests": [...]}` on stdout. Whatever `config` the suite sets is handed to it verbatim, so one script can serve several suites — and, more usefully, the suite still describes what it covers instead of hiding the policy inside the script.

```python
--8<-- "examples/11-test-generators/generate-cases.py"
```

/// note | Generators do not run during `list`

`domarinn list tests` deliberately does not execute generators — listing is meant to be free of side effects. Pass `--generators` when you want their cases enumerated too.

///

---

## Example 12 — Render health

The cheapest useful suite there is: run an external system, grade its output with deterministic assertions, spend nothing. No API key, no model, no network — which makes it safe to run on every pull request.

```yaml
--8<-- "examples/12-render-health/domarinn.yaml"
```

Two things are worth reading closely. The `!raw` tag on the SSTI payload keeps `{{7*7}}` verbatim, so the `not-contains: "49"` assertion is actually testing something. And `defaults.assert` applies a length ceiling to every case in the suite — a cheap catch for a runaway template — without repeating it per case.

---

## Example 13 — Your own system

This is the provider domarinn is built around. An `exec` provider runs **your** program, so the eval exercises the same code path production does — the same rendering, the same client, the same guardrails — instead of a raw model endpoint that resembles it.

```yaml
--8<-- "examples/13-exec-provider/domarinn.yaml"
```

/// warning | Three things that surprise people

1. **`command` paths resolve relative to the suite file's directory**, not the shell's working directory. `domarinn run examples/13-exec-provider` from the repo root and `domarinn run .` from inside it behave identically.
2. **The cache key hashes what the program is sent, not the program's bytes.** Rebuild your binary and the cache still answers with the old results. That is what makes an entry reusable on another machine, and it is why [example 22](#example-22--cache-salts) exists.
3. **There is deliberately no flag that swaps a provider's model.** The model is part of argv, therefore part of the request. To compare two models you write two providers — which is what the example above does, and why the report has two columns.

///

The program itself is ordinary. Read a request, do your thing, write a response:

```python
--8<-- "examples/13-exec-provider/assistant.py"
```

Reporting `usage` is what makes `tokens:` and `cost:` assertions meaningful. Omit it and a `cost:` budget passes as "cost not reported" — green, and enforcing nothing.

---

## Example 14 — A custom assertion

The built-in assertions cover substrings, shapes, schemas and rubrics. When a correctness rule is genuinely *yours* — a checksum, a units conversion, a lookup against a table only you have — an `exec` assertion runs your program and takes its verdict.

```yaml
--8<-- "examples/14-custom-exec-assert/domarinn.yaml"
```

The contract mirrors the provider one, and the request carries the output, the case's vars, and whatever `config` the suite set:

```python
--8<-- "examples/14-custom-exec-assert/check-total.py"
```

Write `reason` for the person reading a red build, not for a log — it is what appears beside the case in the report. And exit `0` whenever you produced a verdict, *including a failing one*: a non-zero exit means the checker itself broke, which domarinn reports as an infrastructure error rather than a test failure.

---

## Example 15 — Tool-call assertions

When an agent's right answer is "call this tool with these arguments", there is no sentence to match. domarinn declares the tools, your provider offers them to the model, and the `tool-call` assertion grades which one it chose.

domarinn never **executes** a tool. It is an evaluation harness, not an agent runtime — what it wants back is the decision, which is the thing under test.

```yaml
--8<-- "examples/15-tool-call-asserts/domarinn.yaml"
```

`args` is a **subset** match: the call may carry more fields, but the ones named must be present and equal. Asserting the whole object would make the test brittle against a harmless extra field. When the exact value is not what you are testing, `schema` shape-checks the arguments instead.

/// danger | Declaring tools changes what you measure

A model offered tools stops writing "I'll look that up for you" and starts emitting a tool call with no prose — so every rubric watching the text channel suddenly sees nothing, and a suite that was green goes red for reasons that have nothing to do with the change you were testing. Add `tools:` when you intend to assert on tool calls, not as background context.

///

The case most often missing is the one where **no** tool should be called. `agent/no-tool-needed` is that case, and it is what catches tool-eagerness.

---

## Example 16 — Tags and filters

A suite grows until running all of it stops being what you want on every change. Three levers cut it down, and they compose:

```console
$ domarinn run examples/16-tags-and-filters --tag safety
$ domarinn run examples/16-tags-and-filters --filter 'billing/*'
$ domarinn run examples/16-tags-and-filters --provider fast
```

```yaml
--8<-- "examples/16-tags-and-filters/domarinn.yaml"
```

Two more live in the suite itself. `only_providers` and `skip_providers` are for cases that are not merely uninteresting elsewhere but *meaningless* elsewhere — `safety/cites-policy` measures a behaviour only one of these providers claims to have, so running it against the other would report a failure that is not a regression. That is why this suite is seven cells and not eight.

Prefer a tag or a filter on the command line for "not right now", and the per-case lists only for "this cannot apply there". A suite whose cases quietly skip themselves is hard to reason about.

---

## Example 17 — Composition

`extends` names one base suite this file is merged on top of; `imports` names fragments merged in order before the local file wins.

```yaml
--8<-- "examples/17-defaults-and-composition/base.yaml"
```

```yaml
--8<-- "examples/17-defaults-and-composition/domarinn.yaml"
```

The merge is a deep merge with one important exception:

- **maps** merge key by key, and the child wins on a conflict;
- **sequences** are replaced wholesale — *except* `assert`, which is **appended**.

That exception is deliberate. Restating `providers` in a child should mean "these providers, not those". Restating `defaults.assert` should **not** silently discard the safety rules the base layer exists to enforce.

/// note | Two different things are called "defaults"

*Within* one file, `defaults` is merged into each test. *Across* files, a shared `assert` sequence is appended, base first. They are easy to conflate and behave differently.

///

---

## Example 18 — A failing gate

Every other example on this page is green, which is a poor way to learn what red means. This one keeps a genuine regression in it, so the failure output, the exit code, and the short-circuit behaviour are documented by something that actually runs.

```yaml
--8<-- "examples/18-failing-gate/domarinn.yaml"
```

`domarinn run examples/18-failing-gate` exits **1**, with one case passing and two failing. The exit codes are the CI contract:

| Code | Meaning |
| ---- | ------- |
| `0`  | Every case passed. |
| `1`  | At least one **assertion** failed — the system under test got worse. |
| `2`  | Usage error — a malformed suite, a bad flag. |
| `3`  | **Infrastructure** error — the harness broke. See [example 19](#example-19--errors-and-retries). |

`1` and `3` are separate on purpose, and `3` wins when both occur. "The model got worse" and "the harness broke" demand different responses, and a gate that conflates them trains people to ignore it.

The third case demonstrates **short-circuiting**. Its deterministic `icontains` fails, so with no threshold the case is already decided and the graded `exec` assertion below it is recorded as `skipped` — no subprocess spawned, no tokens spent. The program behind it exits non-zero on purpose: if short-circuiting ever stopped working, this suite would report exit 3 instead of exit 1, and the change would be impossible to miss.

---

## Example 19 — Errors and retries

A failed assertion means the system under test got worse. An **error** means you learned nothing — the call never produced a gradeable answer. Conflating the two is how a gate starts lying, so domarinn keeps them apart end to end: separate cell status, separate tally, separate exit code.

```yaml
--8<-- "examples/19-errors-and-retries/domarinn.yaml"
```

This suite exits **3**, with one pass, two errors, one skip, and *no* assertion failures at all.

**Retries** apply only to errors the provider marks `retriable: true`. That distinction belongs to the provider because only it knows: a rate limit is transient, a rejected credential never will be. Getting it backwards is expensive in both directions — retrying a bad key hammers an endpoint that will never say yes, and giving up on a 429 throws away a run that would have succeeded a second later.

```python
--8<-- "examples/19-errors-and-retries/flaky.py"
```

**Empty answers** are the subtle one. A blank output is a *successful* call, so nothing upstream raises and every assertion scores zero for a reason unrelated to the prompt. A provider that knows why says so with `empty_reason`, and `runner.skip_on_empty_reason` turns named reasons into skips — so a suite measuring formatting quality is not dragged down by cases the model declined for unrelated reasons.

---

## Example 20 — Runner tuning

Cases are independent, so concurrency changes wall-clock and nothing else — until it changes your results, which is what a rate limit is for.

```yaml
--8<-- "examples/20-runner-tuning/domarinn.yaml"
```

The default concurrency is **1**: deliberately boring, so a first run is reproducible and nobody's first experience of the tool is a wall of 429s. Match `concurrency` to what the system under test can take, not to your core count — the bottleneck is almost always on the other end.

`concurrency` and `rate_limit` are different constraints. Eight concurrent calls that each take a second is 8 rps; the same eight against a fast endpoint could be hundreds.

Commit these in the suite rather than passing `-j` on the command line, so a local run and CI schedule the same way. The flags exist to override for one run, not to carry the configuration.

---

## Example 21 — Caching

Every outgoing request is cached, content-addressed, on by default. Run this suite twice and the second run does no work at all.

```yaml
--8<-- "examples/21-caching-basics/domarinn.yaml"
```

The rule the key follows is one sentence: **hash what is sent.** A provider call, the LLM judge, an embedding and an `exec` grader are all keyed the same way — the SHA-256 of the redacted request, plus the trial index, plus any `cache_salt` in scope. Nothing about your machine, your binary, or your credentials.

That is what makes a cache shareable. A key that varied by machine could not be reused by anyone else, which quietly turns a shared cache into an expensive local disk.

Three consequences worth knowing:

- **One entry per key, immutable.** The first write wins, on every backend.
- **Errors are never cached.** Only successful responses are stored.
- **`latency` assertions bypass the cache entirely**, because a replayed response has no honest latency to report — and under `--cache-only` such a case is refused rather than called live. `cost` and `tokens` come from the stored response.

Note the `cache:` block names only the *kind* of backend. The URL and credentials come from the environment, so a suite stays safe to commit. See [Caching](caching.md#the-rule) for the full rule and the shared backends.

---

## Example 22 — Cache salts

The key is the request, and a request only carries what domarinn can **see**. When the system under test loads its own content across a process boundary — prompts from a registry, rules from a database — domarinn never sees that content, so editing it changes nothing about the request and the cache keeps answering with yesterday's results.

```yaml
--8<-- "examples/22-cache-salts/domarinn.yaml"
```

`cache_salt` is the lever, and it exists at two levels because it is really two problems:

| Level | What it is | Bump it when |
| ----- | ---------- | ------------ |
| **Provider** | A coarse "same build?" version pin. | The program's own logic changes. |
| **Per case** | A content digest of just what *this* case exercises. | Never by hand — `$digest:` computes it. |

Do **not** make the provider-level salt a content digest of everything the program reads. That throws the whole cache away on any edit, which is precisely the outcome the per-case salt exists to avoid. The two-level arrangement is what keeps a large suite affordable while staying honest: edit one prompt and only the cases that use it re-run.

`$digest:` renders its glob against the case's own vars, hashes matched files in sorted order *with their relative paths*, and treats a glob matching nothing as an error — because an empty digest would silently mean "never bust".

---

## Example 23 — Repeat and confidence

A pass rate off one run of twenty cases is a number with no error bar. Models are stochastic, and so is anything built on them: "17/20 passed" and "17/20 passed, 95% CI [0.62, 0.96]" are the same measurement, but only one tells you whether yesterday's 15/20 was a regression or noise.

```yaml
--8<-- "examples/23-repeat-and-confidence/domarinn.yaml"
```

`--repeat N` runs every cell N times, and the report gains three things:

- **Wilson confidence intervals** on the pass rate — well-behaved at small N and at rates near 0 or 1, where the normal approximation is simply wrong.
- **pass@k** — did at least one of k attempts succeed, which is the right question for anything with a retry loop in front of it.
- **McNemar significance** when diffing two runs — a *paired* test, because both runs saw the same cases, and treating them as independent samples throws away exactly the information that makes the comparison sharp.

The trial index is part of the cache key, so repeats genuinely re-sample instead of replaying one cached answer N times.

---

## Example 24 — Baselines and diff

A pass rate on its own cannot tell you whether a change made things worse. `--against` compares a run to a baseline cell by cell and gates on **regressions** — cases that passed before and fail now — so a suite that was 80% green yesterday does not have to be 100% green today to merge.

```yaml
--8<-- "examples/24-baselines-and-diff/domarinn.yaml"
```

/// danger | `--against latest` silently never gates in CI

`latest` resolves through a **cwd-relative** `.domarinn/runs/latest`. A fresh CI checkout has no such directory, so it finds nothing, logs a *warning*, and lets the job exit `0` on a real regression.

It is right for local iteration and useless in CI. Use `--against server:baseline` there — and note that "no baseline pinned" is *also* only a warning, so the gate is never better than what is actually pinned. A stale or partial baseline compares almost nothing and reads as a pass.

///

Baselines are keyed per provider id, so renaming a provider — or changing the model inside its `command` — starts its history over.

---

## Example 25 — Output formats

`--format` is repeatable, so one run can feed a human and a machine at once.

```yaml
--8<-- "examples/25-output-formats/domarinn.yaml"
```

| Format | For |
| ------ | --- |
| `table` | The default. A terminal report with colour. |
| `json` | The full result document — every cell, assertion and token count. |
| `jsonl` | One JSON object per line, for streaming into a log pipeline. |
| `junit` | XML every CI system already knows how to render. |
| `md` | Markdown, for a PR comment or a job summary. |

`--out FILE` takes a **single** path, so one invocation writes one machine format to a file; producing both JSON and JUnit is two invocations. `--summary-md FILE` is separate and can accompany either — it is what you point at `$GITHUB_STEP_SUMMARY`.

Colour follows `NO_COLOR` and `CLICOLOR_FORCE`, and the machine formats are never coloured regardless, so piping `json` into `jq` never surprises you with escape codes. Logs always go to stderr, so stdout stays parseable.

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

The cache key is the request this provider would send: the rendered `method`, `url` and `body`, plus a digest of the rendered `headers`. A `${…}` placeholder your own backend interprets is left untouched — only the `${env:…}` sigil is claimed. `output_expr` projects the *response*, so it is not in the key: change it and re-run with `--no-cache`.

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

It needs a `type: embeddings` provider in the suite; without one the assertion **errors** rather than passing. Verdicts are cached against the embedding model, so changing the model re-embeds everything — as it must, since cosines are not comparable across models.

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

## See also

- [Getting started](getting-started.md) — install, then write and run your first suite.
- [Suite configuration](configuration.md) — the complete `domarinn.yaml` reference.
- [Assertions](assertions.md) — every assertion type, weights, thresholds, and short-circuiting.
- [Providers](providers.md) — `exec`, `http`, `anthropic`, `openai`, and `embeddings`.
