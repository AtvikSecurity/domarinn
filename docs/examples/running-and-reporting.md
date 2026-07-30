# Running & reporting

Seven suites about operating a suite once it exists: narrowing what runs with tags and filters, composing configuration across files, reading the exit-code contract a red build produces, telling an error apart from a failure, tuning concurrency against a real rate limit, shaping the report for a human or a machine — and arriving from promptfoo with a config you already have. Read these when you are wiring a suite into a script or a CI job.

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

## Example 39 — A promptfoo config, converted

If you already have a promptfoo suite, the first domarinn suite you run can be that one. `domarinn import promptfoo` translates what has a faithful equivalent and leaves a `# NOTE:` for what does not, so nothing disappears quietly.

This example ships both halves of one migration. The promptfoo config going in, and a walk through the conversion, are in the [migration guide](../guides/migrate-promptfoo.md#a-worked-conversion); here is the suite that came out — the converter's own output rather than a hand-written suite, header and sequence indentation aside:

```yaml
--8<-- "examples/39-import-promptfoo/domarinn.yaml"
```

Two of that config's assertions did not survive, and the notes name both: `not-icontains` is not a type the converter recognises — domarinn supports the spelling, the converter matches on the bare name — and `javascript` has no equivalent at all, its replacement being [an `exec` assertion](your-own-system.md#example-14--a-custom-assertion). Re-adding them is the part of a migration a converter cannot do for you.

The ids are the converter's too: `p0` for the provider, and `case-0` / `case-1` for promptfoo cases that carried no description. They run as they are, and they are the first thing worth renaming — an id is what `--provider`, `only_providers` and a baseline diff refer to. A converted suite also has no `project:` or `suite:` name, which is what groups and compares runs on the results server.

CI converts the shipped promptfoo config, compares the result against the committed file, and runs both, so this page cannot show a conversion that no longer happens.
