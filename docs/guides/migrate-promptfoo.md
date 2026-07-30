# Migrating from promptfoo

domarinn ships a converter:

```console
$ domarinn import promptfoo promptfooconfig.yaml > domarinn.yaml
```

It prints YAML to stdout and leaves a `# NOTE:` comment wherever an assertion or a provider did not map. Read those before running anything — they are the parts that need a decision, not a translation.

/// tip | Convert, then run `validate`

```console
$ domarinn import promptfoo promptfooconfig.yaml > domarinn.yaml
$ domarinn validate .
$ domarinn list tests .
```

`validate` catches shape problems without calling anything, and `list tests` shows you what the suite actually resolves to — worth checking before a first run spends money.

///

## A worked conversion

Both halves of one conversion ship in the repository as [example 39](../examples/running-and-reporting.md#example-39--a-promptfoo-config-converted), and both are run in CI. The promptfoo config going in:

```yaml
--8<-- "examples/39-import-promptfoo/promptfooconfig.yaml"
```

And the suite coming out — the converter's own output rather than a hand-written suite, with a header on top naming the two cosmetic things that were added:

```yaml
--8<-- "examples/39-import-promptfoo/domarinn.yaml"
```

Read the two `# NOTE:` lines first, because they are the whole reason to look at a conversion before running it. One assertion was a `javascript` check, which has no equivalent — the replacement is [an `exec` assertion](../examples/your-own-system.md#example-14--a-custom-assertion). The other was `not-icontains`, a spelling domarinn accepts everywhere but the converter does not recognise, so a house safety rule left the suite. Both have to be re-added by hand; neither was dropped in silence.

What did convert is worth reading for what it does *not* decide for you. `exec:../echo-provider.py` became a provider block with the id `p0`, the two promptfoo cases became `case-0` and `case-1`, and the suite carries no `project:` or `suite:` name at all — the converter cannot invent names, and those are the strings that group runs on a results server and that `--provider`, `only_providers` and a baseline diff refer to. Rename them before the ids reach a stored baseline.

CI re-runs the converter on that config, compares the output against the committed suite, then runs both, so the pair above can only fall out of step if the converter itself changes.

## What maps cleanly

| promptfoo | domarinn |
| --------- | -------- |
| `providers: [openai:gpt-4o]` | a `type: openai` block with `model: gpt-4o` |
| `providers: [anthropic:messages:claude-…]` | a `type: anthropic` block |
| `prompts:` (a plain string, inline or `file://`) | `prompts:` with a single-string `template:` |
| `tests:` with `vars` | `tests:` with `vars` |
| `defaultTest` | `defaults` |
| `contains`, `icontains`, `regex`, `equals`, `starts-with` | the same, kebab-case |
| `is-json`, `contains-json` | the same |
| `llm-rubric` | `llm-rubric`, with the grader configured in a `grader:` block |
| `similar` | `similar`, needing a `type: embeddings` provider in the suite |
| `threshold`, `weight` | the same |

## What changes, and why

**Provider id strings become blocks.** There is no `openai:gpt-4o` shorthand. Every provider is a config block with an `id` and a `type`. It is more to write, and it is what lets two providers differ by `base_url`, `params` or `pricing` without inventing more string syntax — and what makes `--provider` and `only_providers` refer to something you named.

**Templating is Jinja, not Nunjucks.** `{{ var }}` is the same. Filters mostly are not: domarinn ships `json_encode`, `b64encode`, `sha256`, `slugify`, `regex_replace`, `truncate` and friends, plus `now()`, `uuid()` and `randint()`. Check any filter you rely on.

**A converted prompt is always a `template:`, never `messages:`.** The converter reads a promptfoo prompt that is a **plain string** and emits one single-string template. A prompt written as anything else — an object with `id` / `label` / `raw`, or an inline list of chat messages — is skipped, and skipped without even a `# NOTE:`, so count the `prompts:` you get against the ones you had. domarinn's chat-transcript form — a system turn, prior turns, then the newest one — is [a `messages:` prompt](../reference/domarinn-yaml.md#prompts), and it has to be written by hand. A `file://` string is carried across as a `file://` template, which domarinn does load from disk — but as a *template*, and relative to the *suite* directory, refusing anything outside it: a prompt file that lived above your promptfoo config has to move in beside the suite, and a promptfoo prompt **function** (`file://prompt.py:fn`) arrives as a path with nothing in domarinn to call it.

**`not-` prefixed assertions are supported, but not converted.** domarinn accepts `not-contains`, `not-icontains` and every other `not-<kind>` spelling, in inline cases and in every loaded test format. The converter does not: it matches on the bare type name, so a `not-` assertion is reported as unmapped and left out of the output. The note names it, and the fix is to paste the assertion back unchanged.

**JavaScript and Python assertions do not exist.** There is no `javascript:` or `python:` assertion type. The replacement is [an `exec` assertion](../examples/your-own-system.md#example-14--a-custom-assertion) — your program, any language, over a small JSON protocol. It is a process boundary rather than an embedded interpreter, which costs a spawn and buys you a checker you can run and test on its own.

**Some graded assertion types have no equivalent.** `moderation`, `factuality`, `answer-relevance`, `select-best`, `perplexity`, `rouge`, `levenshtein` and `classifier` are not implemented. Most are expressible as an `llm-rubric` with an explicit rubric, which is more work to write and considerably easier to reason about when it disagrees with you.

**There is no per-assertion `transform` or `metric`.** Shape the output in your provider, where you can test it.

**Output format is a flag, not config.** `--format json|jsonl|junit|md|table`, `--out`, `--summary-md`. There is no `outputPath` key.

**`--no-cache` is rarely what you want.** promptfoo's cache keyed on things that made sharing impractical, so `--no-cache` became habit. domarinn's key is [portable by construction](../concepts/how-a-run-works.md#the-cache-key-is-the-request-not-the-machine) — it is a hash of the request and nothing else — so the cache is shareable across a team and CI. Reach for `cache_salt` instead of disabling caching, or `--no-grader-cache` when it is only the grader you want re-asked.

## What you gain

Worth knowing about, because they have no promptfoo equivalent and change how a suite is used:

- **[Regression gating](../examples/caching-and-statistics.md#example-24--baselines-and-diff)** against a stored baseline, with a defined exit-code contract — `1` for a failed assertion, `3` for a broken harness.
- **[Statistics](../concepts/statistics.md)** — Wilson confidence intervals, McNemar paired significance, pass@k. A pass rate with an error bar.
- **[Errors that are not failures](../examples/running-and-reporting.md#example-19--errors-and-retries)** — a separate status, tally and exit code, so "the harness broke" never reads as "the model got worse".
- **[A fail-closed grader](../examples/models-grading-and-budgets.md#example-29--llm-rubric-grading)** — structured verdicts, with truncation an error rather than a zero.
- **A [self-hostable results server](../reference/server.md)** with accounts, run comparison and a shared cache, in the same single binary.

## See also

- [Your first eval](../start/first-eval.md) — if you would rather write a fresh suite than convert one.
- [domarinn.yaml](../reference/domarinn-yaml.md) — the complete reference.
- [Examples](../examples/index.md) — the capability ladder.
