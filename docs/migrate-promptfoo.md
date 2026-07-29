# Migrating from promptfoo

domarinn ships a converter:

```console
$ domarinn import promptfoo promptfooconfig.yaml > domarinn.yaml
```

It prints YAML to stdout and leaves a `# NOTE:` comment wherever something did not map. Read those before running anything — they are the parts that need a decision, not a translation.

/// tip | Convert, then run `validate`

```console
$ domarinn import promptfoo promptfooconfig.yaml > domarinn.yaml
$ domarinn validate .
$ domarinn list tests .
```

`validate` catches shape problems without calling anything, and `list tests` shows you what the suite actually resolves to — worth checking before a first run spends money.

///

## What maps cleanly

| promptfoo | domarinn |
| --------- | -------- |
| `providers: [openai:gpt-4o]` | a `type: openai` block with `model: gpt-4o` |
| `providers: [anthropic:messages:claude-…]` | a `type: anthropic` block |
| `prompts:` (inline or `file://`) | `prompts:` with `template:` or `messages:` |
| `tests:` with `vars` | `tests:` with `vars` |
| `defaultTest` | `defaults` |
| `contains`, `icontains`, `regex`, `equals`, `starts-with` | the same, kebab-case |
| `is-json`, `contains-json` | the same |
| `llm-rubric` | `llm-rubric`, with the grader configured in a `grader:` block |
| `similar` | `similar`, needing a `type: embeddings` provider in the suite |
| `not-` prefixes | the same |
| `threshold`, `weight` | the same |

## What changes, and why

**Provider id strings become blocks.** There is no `openai:gpt-4o` shorthand. Every provider is a config block with an `id` and a `type`. It is more to write, and it is what lets two providers differ by `base_url`, `params` or `pricing` without inventing more string syntax — and what makes `--provider` and `only_providers` refer to something you named.

**Templating is Jinja, not Nunjucks.** `{{ var }}` is the same. Filters mostly are not: domarinn ships `json_encode`, `b64encode`, `sha256`, `slugify`, `regex_replace`, `truncate` and friends, plus `now()`, `uuid()` and `randint()`. Check any filter you rely on.

**JavaScript and Python assertions do not exist.** There is no `javascript:` or `python:` assertion type. The replacement is [an `exec` assertion](examples.md#example-14--a-custom-assertion) — your program, any language, over a small JSON protocol. It is a process boundary rather than an embedded interpreter, which costs a spawn and buys you a checker you can run and test on its own.

**Some graded assertion types have no equivalent.** `moderation`, `factuality`, `answer-relevance`, `select-best`, `perplexity`, `rouge`, `levenshtein` and `classifier` are not implemented. Most are expressible as an `llm-rubric` with an explicit rubric, which is more work to write and considerably easier to reason about when it disagrees with you.

**There is no per-assertion `transform` or `metric`.** Shape the output in your provider, where you can test it.

**Output format is a flag, not config.** `--format json|jsonl|junit|md|table`, `--out`, `--summary-md`. There is no `outputPath` key.

**`--no-cache` is rarely what you want.** promptfoo's cache keyed on things that made sharing impractical, so `--no-cache` became habit. domarinn's key is [portable by construction](concepts/how-a-run-works.md#the-cache-key-is-the-request-not-the-machine) — it deliberately contains nothing about your filesystem — so the cache is shareable across a team and CI. Reach for `cache_salt` instead of disabling caching.

## What you gain

Worth knowing about, because they have no promptfoo equivalent and change how a suite is used:

- **[Regression gating](examples.md#example-24--baselines-and-diff)** against a stored baseline, with a defined exit-code contract — `1` for a failed assertion, `3` for a broken harness.
- **[Statistics](statistics.md)** — Wilson confidence intervals, McNemar paired significance, pass@k. A pass rate with an error bar.
- **[Errors that are not failures](examples.md#example-19--errors-and-retries)** — a separate status, tally and exit code, so "the harness broke" never reads as "the model got worse".
- **[A fail-closed grader](examples.md#example-29--llm-rubric-grading)** — structured verdicts, with truncation an error rather than a zero.
- **A [self-hostable results server](server.md)** with accounts, run comparison and a shared cache, in the same single binary.

## See also

- [Getting started](getting-started.md) — if you would rather write a fresh suite than convert one.
- [Suite configuration](configuration.md) — the complete reference.
- [Examples](examples.md) — the capability ladder.
