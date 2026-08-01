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

Example 02 already used a `messages:` prompt — system, then one user turn. This one adds what an actual back-and-forth needs: a prior ASSISTANT turn too, fixed across every case, with only the newest user turn templated per case. That is what a real follow-up question looks like — the model needs to see what it already said, not just the newest line.

```yaml
--8<-- "examples/34-multi-turn-conversation/domarinn.yaml"
```

Every turn is a template, so a fixed turn simply has nothing in it to substitute — only the last one varies per case. Both cases above share the first three turns byte for byte and differ only in the follow-up, which is the thing under test.

The echo provider makes this observable: an exec provider receives a `messages:` prompt as `{"messages": [...]}` (see [the protocol](../reference/protocol.md)), and echoing it back is what lets a `contains` assertion prove the whole rendered history — not just the last var — actually reached the provider.

---

## Example 41 — Per-case conversation history

Example 34 fixes the transcript in the prompt: every case shares the same prior turns. This one moves the history to the cases — each brings its own prior turns, different lengths and roles included, and the prompt names where they splice in with the bare `history` marker entry:

```yaml
--8<-- "examples/41-per-case-history/domarinn.yaml"
```

The `history/escalation` case points at a transcript file instead of inlining it — the same shape, a YAML list of turns:

```yaml
--8<-- "examples/41-per-case-history/convos/escalation.yaml"
```

And the CSV rows carry theirs in the reserved `__history` column, JSON-encoded (or a `file://` path). An empty cell means *unset* — with a `defaults.history` in play it would inherit the default; a literal `[]` cell is the opt-out:

```csv
--8<-- "examples/41-per-case-history/cases.csv"
```

Three details worth knowing. A prompt may hold at most one marker, and a prompt *without* one still works: each case's history lands right after the prompt's leading system turn(s), and a `template:` prompt becomes the transcript's newest user turn. Second, history joins each case's request identity — two cases differing only in their prior turns key, and cache, separately. Third, the `not-contains` assertions above are the point: each case's transcript is its own, and nothing from another case's conversation leaks in.

See [per-case history in the reference](../reference/domarinn-yaml.md#per-case-history) for the full splice rules.

---

## Example 42 — Replaying a tool-using transcript

Example 41 gives each case its own prior turns, but only as prose — and that is the wrong shape for the case `history` is most useful for. An agent told to "call `lookup_order` first" will, in a single-turn eval, call it and stop: nothing ever feeds a result back, so the decision you actually wanted to grade never happens. History is what supplies turn one's result. But turn one *is* a tool call, and turn two's input *is* a tool result:

```yaml
--8<-- "examples/42-tool-call-history/domarinn.yaml"
```

An `assistant` turn's `tool_calls` use the same `{id, name, arguments}` shape a provider *reports* on the way out, so the `tool_calls` block of a stored case pastes straight into a suite. A `tool` turn carries the result, naming the call it answers with `tool_call_id` — optional, because position pairs them when a transcript omits it, which is what makes the `tools/parallel-round` case above work without a single id written anywhere.

A whole tool-using trace can live in a file, which is usually where one captured from a real agent run already is:

```yaml
--8<-- "examples/42-tool-call-history/convos/lookup.yaml"
```

Three details. Arguments are templated leaf by leaf, so `{order_id: 1042}` stays an integer — stringifying the value to template it would make it `"1042"`, which a [`tool-call` assertion](../reference/assertions.md#tool-call) comparing against the decoded object would never match. A turn's `content` may be a list of typed blocks instead of a string, which is how `thinking` is replayed; that one is **never** templated, because its `signature` is a vendor integrity token over those exact bytes. And the two vendors disagree in mirrored ways about a round of parallel results — `anthropic` folds them into one user message, `openai` sends one message each — which domarinn handles so the suite does not have to say.
