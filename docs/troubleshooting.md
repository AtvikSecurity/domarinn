# Troubleshooting

Symptoms, causes, and fixes. Every entry here is something that actually caught someone out — most of them are cases where the tool did exactly what it was told and the result still surprised the person who told it.

## The gate is green but nothing is being checked

### `--against latest` never fires in CI

**Symptom.** A regression gate reports success on a pull request that plainly made things worse. Locally the same command catches it.

**Cause.** `latest` resolves through a **cwd-relative** `.domarinn/runs/latest`. A fresh CI checkout has no `.domarinn/` at all, so it finds nothing, logs a *warning*, and lets the job exit `0`.

**Fix.** Use `--against server:baseline` in CI. And know that "no baseline pinned" is *also* only a warning — so the gate is never better than what is actually pinned. A stale or partial baseline compares almost nothing and reads as a pass. Check what your baseline actually contains before trusting a green run.

### A `cost:` budget that enforces nothing

**Symptom.** `- type: cost, max: 0.01` passes on a run you know was expensive.

**Cause.** When nothing priced the call, the assertion passes with the note *"cost not reported; budget not enforced"*. That happens when the provider reports no `usage`, or when the model is not in domarinn's rate sheet and the suite sets no `pricing:` block.

**Fix.** Set a [`pricing:` block](examples/models-grading-and-budgets.md#example-27--anthropic-and-what-it-costs) on the provider, or have your `exec` provider report `cost_usd` directly. `tokens:` has the same failure mode and the same fix. Check `summary.cost_usd` is non-null before believing a cost gate.

### A `similar` assertion that passes at any threshold

**Symptom.** Every case clears `similar`, including ones that obviously should not.

**Cause.** Usually a stubbed or mocked embeddings endpoint returning one fixed vector. The cosine of a vector with itself is `1.0`, so the threshold is never exercised.

**Fix.** Also note the default threshold is `0.8`, which is looser than most people expect — unrelated sentences in the same domain routinely clear `0.7`. State a threshold you chose.

---

## Templating

### A test input containing `{{ … }}` was evaluated

**Symptom.** A case built to prove the system does *not* evaluate template syntax passes — but for the wrong reason, because the payload was already evaluated before it got there.

**Cause.** Every var is rendered through Jinja. `{{7*7}}` becomes `49` on the way in.

**Fix.** Tag the value `!raw`, or use the `{$raw: …}` object form in JSON/JSONL/CSV where a YAML tag cannot travel. See [example 06](examples/templates-and-test-data.md#example-06--the-raw-escape-hatch), which keeps a control case proving the untagged form really is rendered.

/// danger | This failure is silent

The assertion still passes. Nothing warns you. The test simply stops testing what its name says.

///

### `!raw` breaks another tool that reads the suite

**Symptom.** A linter, a formatter or a CI script that parses `domarinn.yaml` fails with an unknown-tag error.

**Fix.** Give that tool a `!`-tag passthrough loader. domarinn's own YAML tags are not standard, and a naive `yaml.safe_load` will refuse them.

### `output.startswith(...)` errors in a `jinja` assertion

**Cause.** It is minijinja, not Python. Strings have no `.startswith()` method.

**Fix.** Use the `in` operator (`'score=' in output`) or a filter (`output | length`, `output | trim`). An expression that errors is recorded as an assertion **error**, not a quiet failure.

---

## Caching

### Editing a prompt changed nothing

**Symptom.** You edit a prompt your system loads by id, re-run, and get byte-identical results instantly.

**Cause.** [The key is the request](concepts/caching.md#the-rule), and the request carries only an id. When your program resolves its own prompts across a process boundary, domarinn never sees that content, so nothing in the key changed.

**Fix.** The [two-level salt pattern](examples/caching-and-statistics.md#example-22--cache-salts). A coarse `cache_salt` on the provider as a build pin, and a per-case `cache_salt: "$digest: prompts/{{ prompt_id }}.md"` so editing one prompt busts only the cases that use it. Do **not** make the provider-level salt a content digest of everything the program reads — that discards the whole cache on any edit, which is what the per-case salt exists to avoid.

### A rebuild did not re-run anything

**Cause.** The key hashes what an `exec` provider *sends*, not the bytes of the program that sends it. That is deliberate: it is what makes a cache entry reusable on another machine. See [`exec` providers and the provider salt](concepts/caching.md#exec-providers-and-the-provider-salt).

**Fix.** Bump the provider's `cache_salt` when a rebuild should discard old answers. domarinn warns when it replays cached answers for a provider whose program may have changed.

### `--cache-only` fails a case with "there is nothing honest to replay"

**Cause.** That case has a `latency` assertion. `latency` always measures a live call, and `--cache-only` promises not to make one — so the case is refused instead of quietly reaching the provider. Only that case fails; the rest of the suite replays.

**Fix.** Drop `--cache-only` for that suite, or filter the latency cases out of the offline run (`--filter`, `--tag`). See [cache modes](concepts/caching.md#cache-modes).

### `--cache-only` fails every `similar` assertion after upgrading

**Cause.** 0.4.x stored one cosine value per `similar` assertion; 0.5.0 caches the two embedding vectors instead, and a cosine decomposes into neither — so those entries are the one shape that is deliberately **not** adopted forward.

**Fix.** Run the suite once in the default read-write mode to lay the vectors down. It costs a fraction of a cent and is warm from then on. See [Upgrading to 0.5](concepts/caching.md#upgrading-to-05).

### The examples in this repo write into the repo

**Cause.** The cache defaults to `<suite dir>/.domarinn/cache`, and `.gitignore` hides it — so a stale cache silently satisfies the next run.

**Fix.** Pass `--cache-dir` when running a suite you do not own, or `--no-cache` for a one-off.

---

## Providers

### `command` cannot find my program

**Cause.** `command` paths resolve relative to the **suite file's directory**, not the shell's working directory.

**Fix.** Write them relative to the suite. `domarinn run evals/my-suite` from the repo root and `domarinn run .` from inside `evals/my-suite` then behave identically.

### There is no flag to swap a provider's model

This is deliberate, not an omission. The model is part of the request — for an `exec` provider, part of `command` — so a flag that changed it would silently re-key every entry.

**Do this instead:** declare a second provider and scope a run with `--provider`. Note that baselines are keyed per provider id, so changing a provider's model *does* start its history over.

### Every grade dies on 401 but completions succeed

**Cause.** The grader reads **only** what its own `api_key_env` names. It does not inherit the provider's credential resolution.

This fails asymmetrically, which is what makes it confusing: the system under test answers fine, every grade errors, and the run looks like an infrastructure fault rather than a credential one.

**Fix.** Set the variable the grader's `api_key_env` names. If you are resolving a key from several possible variables in a task runner, re-export it under the name the grader expects.

### The grader errors with a truncated verdict

**Cause.** A default `max_tokens` can cut off a thinking model mid-verdict. domarinn treats that as fail-closed — an `error`, never a silent pass.

**Fix.** Raise `max_tokens` in the grader's `params`. A generous ceiling costs nothing, because you are billed for tokens actually generated. Note also that some models reject non-default sampling params entirely (a `temperature` returns 400), so a grader block often carries no `temperature` at all.

---

## Results and reporting

### Declaring `tools:` made everything fail

**Cause.** A model offered tools stops writing prose and starts emitting tool calls. Every rubric watching the text channel now sees nothing, and cases that were green go red for reasons unrelated to whatever you were testing.

**Fix.** Add `tools:` when you intend to assert on tool calls, not as background context. Assert with `tool-call`, not with a prose rubric. See [example 15](examples/your-own-system.md#example-15--tool-call-assertions).

### A refusal is scoring zero and dragging down the suite

**Cause.** A blank output is a *successful* call. Nothing raises, and every assertion scores zero for a reason that has nothing to do with what you were measuring.

**Fix.** Have the provider report `empty_reason` (`refusal`, `truncated`, `tool_use_only`, …), then list the ones you want excluded in `runner.skip_on_empty_reason`. Those cells become `skip` rather than `fail`. See [example 19](examples/running-and-reporting.md#example-19--errors-and-retries).

### The reusable Action posts a stub comment on a green run

**Cause.** The action shells out to `domarinn ci-summary`, which does not exist in older binaries. A mismatch between the action's pin and the CLI version degrades **silently**.

**Fix.** Pin the action and `DOMARINN_VERSION` as a pair, and bump them together.

## See also

- [How a run works](concepts/how-a-run-works.md) — the model behind most of the above.
- [Caching](concepts/caching.md) — the full key semantics.
- [CI integration](ci.md) — exit codes and gating.
- [Examples](examples/index.md) — every entry here has a runnable counterpart.
