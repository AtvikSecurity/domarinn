# First steps

The five suites below are the fastest way to get a working mental model: what a run actually is, how a prompt gets filled in per case, which assertions cost nothing, and how a case turns a set of scores into a single pass or fail. None of them need a model, an API key, or even a network connection. Start here if this is your first domarinn suite.

---

## Example 01 — Hello, eval

The smallest thing that works. One provider, one case, one assertion.

The system under test is a shell one-liner that prints a fixed answer, so this suite needs no model, no API key, and not even Python — just a POSIX shell. Read it as three answers: `providers` is **what** is being tested, `tests` is **which** inputs it gets, and `assert` is **what must be true** of the answer.

```yaml
--8<-- "examples/01-hello-eval/domarinn.yaml"
```

An `exec` provider is any program that reads one JSON request on stdin and writes one JSON response on stdout. That is the entire contract — see the [exec protocol](../protocol.md). Swap the `command` for your own program and this suite is already testing your system.

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
