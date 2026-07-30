# Templates & test data

Seven suites about getting the input side of a suite right before you worry about grading. They cover the one sharp edge in Jinja templating, pulling a var's value from a file instead of writing it inline, fanning one case out across every combination of a matrix, reading — or generating — cases from something other than hand-written YAML, and giving a provider a whole conversation instead of one string. Reach for these once a suite has outgrown a handful of inline cases.

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

Every var goes through Jinja. A fixture containing `{{7*7}}` renders as `49` unless you mark it raw — which silently destroys the premise of any test whose whole point is that the payload stayed literal. See [domarinn.yaml](../reference/domarinn-yaml.md).

///

---

## Example 08 — Matrix sweeps

One case fans out over the cartesian product of its axes, producing one concrete case per combination. Each axis value is merged into `vars`, where it wins over a base var of the same name.

The ids are deterministic — `greet[style=terse,temperature=0]` and friends — which is what lets [`domarinn diff`](../concepts/statistics.md) line two runs up cell by cell. When the generated shape is unwieldy, `matrix_id` renders a friendlier one against the axis values.

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

## Example 34 — A multi-turn conversation

Every example above uses a `template:` prompt — one string. `messages:` renders a whole transcript instead: system, a prior user turn, a prior assistant turn, then the newest one. That is what a real follow-up question looks like — the model needs to see what it already said, not just the newest line.

```yaml
--8<-- "examples/34-multi-turn-conversation/domarinn.yaml"
```

Every turn is a template, so a fixed turn simply has nothing in it to substitute — only the last one varies per case. Both cases below share the first three turns byte for byte and differ only in the follow-up, which is the thing under test.

The echo provider makes this observable: an exec provider receives a `messages:` prompt as `{"messages": [...]}` (see [the protocol](../reference/protocol.md)), and echoing it back is what lets a `contains` assertion prove the whole rendered history — not just the last var — actually reached the provider.
