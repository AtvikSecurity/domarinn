---
name: author-evals
description: Use when writing, running, or debugging domarinn eval suites locally - "add an eval for X", "write a test case", "why is my suite failing", "run the evals", "set up eval CI". Covers the domarinn.yaml schema and the domarinn CLI.
---

# Authoring and running domarinn eval suites

This skill is about the `domarinn` **CLI** — writing suites and running them locally or in CI. For
reading eval history off a server, use the `triage-evals` skill and the `domarinn` MCP tools.

## Suite shape

A suite is a `domarinn.yaml`. The matrix is `providers × prompts × tests × repeat`; every cell
becomes one case.

```yaml
# yaml-language-server: $schema=https://raw.githubusercontent.com/AtvikSecurity/domarinn/main/domarinn.schema.json
version: 1
project: my-project
suite: my-suite

providers:
  - id: gpt
    type: openai
    model: gpt-4o-mini
    params: { temperature: 0 }

tests:
  - id: greeting/polite
    vars:
      user_input: "hello"
    assert:
      - type: icontains
        value: "hi"
```

**Always add the `$schema` comment.** It gives editors completion and validation over the whole
schema, which is far more reliable than recalling field names — including yours.

Provider types are `openai`, `anthropic`, `http`, and `exec`. `exec` runs a local command speaking
the exec protocol (see `docs/reference/protocol.md`) and is how you evaluate something that is not an LLM API.

## Writing assertions that mean something

Assertion types live in `docs/reference/assertions.md`; read it before inventing one. The judgment calls:

- **Prefer deterministic assertions.** `contains`, `icontains`, `regex`, `is-json`, `json-schema`,
  and `latency` cost nothing and never drift. Reach for `llm-rubric` only when correctness is
  genuinely a matter of judgment.
- **A rubric is a grader, and graders have their own failure modes.** Give it explicit criteria,
  not "is this good". Budget its `max_tokens` generously — a truncated verdict scores zero and
  looks like a model failure.
- **Assert the failure too.** A suite where everything passes on day one is usually a suite whose
  assertions are too loose. Include cases you expect to fail, and check that they do.
- **`tags` are how you slice later.** Tag adversarial cases, slow cases, and cost-heavy cases as
  you write them; retrofitting tags across a large suite is tedious.

## Running

```bash
domarinn run                          # the suite in the current directory
domarinn run --tag adversarial        # one slice
domarinn run --filter 'greeting/*'    # glob over test ids
domarinn run --repeat 5               # variance: 5 trials per cell
domarinn run --against latest         # fail on regression vs the previous run of this suite
domarinn run --share                  # upload to the configured server when it finishes
```

Exit codes are a contract, and CI depends on them: `0` all pass, `1` assertion failure or
regression, `2` config/usage error, `3` infrastructure error. **`3` beats `1`** — that is what lets
CI distinguish "the model got worse" from "the harness broke". Do not paper over a `3` by
retrying; find out what failed.

Other commands: `domarinn runs` (local history), `domarinn view <run>` (re-render a run),
`domarinn diff <base> <head>` (compare two runs), `domarinn ci-summary` (markdown for a PR
comment), `domarinn share <run>` (upload).

Run references are uniform across all of them: `latest`, a run id, a `result.json` path, or a run
directory. Runs persist under `.domarinn/runs/<run_id>/result.json`; `DOMARINN_RUNS_DIR` moves the
store.

## The caching model, which explains most surprises

One rule: every outgoing request — the provider call, the LLM judge, an embedding, an `exec` grader
— is keyed on the request itself, plus the trial index, plus any `cache_salt` in scope. Consequences
worth internalizing before you debug a "wrong" result:

- A fully replayed run can still fail, because the _stored grading_ is what failed.
- Editing a prompt changes the provider request, so it forces fresh calls. Editing an **assertion**
  changes only the grading request: provider responses stay cached, and just the judge is re-asked.
- `--no-cache` bypasses everything; `--no-grader-cache` re-grades against cached responses, which is
  what you want after changing a rubric.
- `cache_salt` on a provider is the deliberate escape hatch when you need to force a miss — a
  version pin for a program whose bytes the key does not see.

## Debugging a failing case

1. `domarinn view latest --failed` — the failures, without the noise.
2. Read the assertion reason, not just the status.
3. Check `stop_reason`. `length` means truncated, not wrong — raise `max_tokens`.
4. Check whether it errored rather than failed. An `error_class` is an infrastructure problem.
5. `domarinn run --repeat 5 --filter '<that test id>'` — if it alternates, it is flaky, and the
   fix is the assertion or the temperature, not the prompt.

## CI

Run the suite, gate on the exit code, upload the result, and post the summary:

```yaml
- run: domarinn run --against server:baseline --share --summary-md summary.md
- run: domarinn ci-summary --out $GITHUB_STEP_SUMMARY
  if: always()
```

`--against server:baseline` compares against the suite's server-side baseline rather than whatever
ran last locally, which is the only comparison that means the same thing on every machine.
