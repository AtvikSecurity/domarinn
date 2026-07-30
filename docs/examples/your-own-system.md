# Your own system

Five suites about testing what you actually ship, not a model that merely resembles it. They cover the `exec` provider that runs your own program end to end, writing an assertion for a correctness rule only you can express, grading a tool call instead of a sentence of prose, and the exec protocol itself in a language other than Python. Read these once you are ready to point domarinn at your own code.

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
2. **The cache key hashes what the program is sent, not the program's bytes.** Rebuild your binary and the cache still answers with the old results. That is what makes an entry reusable on another machine, and it is why [example 22](caching-and-statistics.md#example-22--cache-salts) exists.
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

## Example 37 — The exec protocol, in bash

Every other provider in this ladder is Python, because it ships with the fewest assumptions about your machine. The protocol itself does not care: anything that can read one JSON document from stdin and write one to stdout qualifies. Here it's bash and `jq`.

```yaml
--8<-- "examples/37-exec-provider-bash/domarinn.yaml"
```

```bash
--8<-- "examples/37-exec-provider-bash/provider.sh"
```

Three things worth reading in that script. It reads `DOMARINN_PROTOCOL` before doing anything else — the version domarinn is speaking, so a program that supports more than one can branch on it. `jq -r '.vars.user_input // ""'` unwraps the JSON string and supplies a fallback, because an exec provider is never guaranteed any particular var. And the reply is built with `jq -cn` rather than piping `request` back through a filter, so nothing already consumed can leak into the response by accident.
