# Examples

A ladder of complete, runnable suites, each one demonstrating a single capability. Copy any of them, point the provider at your own system, and `domarinn run`.

/// info | Every suite in these pages is executed in CI

The YAML on the example pages is not a transcription. It is pulled at build time from the real directories under [`examples/`](https://github.com/AtvikSecurity/domarinn/tree/main/examples), and every one of those directories is run end to end by `crates/domarinn-cli/tests/examples.rs` — which asserts the exit code, the pass/fail tally, and the exact case ids each suite produces. A page cannot document a suite that no longer works, because the page and the test read the same bytes.

///

/// tip | The mental model

A run is a grid. domarinn takes the **providers** (the systems under test), the **prompts** (optional — a provider may build its own input), and the **tests** (the units you author), and evaluates one **cell** per combination. Each cell calls its provider once and grades the answer against that test's **assertions**.

Deterministic assertions run first and can short-circuit the expensive ones, so a case that fails `contains` never pays for an LLM grader. Each evaluated cell is a case, and a case ends `pass`, `fail`, `error` (the harness broke — never counted as an assertion failure), or `skip`.

///

The examples below are grouped into six pages, roughly in the order you would reach for them:

| Group | Covers |
| --- | --- |
| [First steps](first-steps.md) | The smallest suite that runs, up through weights and thresholds. |
| [Templates & test data](templates-and-test-data.md) | The `!raw` escape hatch, file vars, matrix sweeps, datasets, generators, and multi-turn prompts. |
| [Your own system](your-own-system.md) | The `exec` provider, a custom assertion, tool-call grading, and the protocol itself in bash. |
| [Running & reporting](running-and-reporting.md) | Tags and filters, composition, exit codes, errors and retries, runner tuning, output formats, and importing a promptfoo config. |
| [Caching & statistics](caching-and-statistics.md) | The one cache-key rule, salts, repeat and confidence, baselines and diff. |
| [Models, grading & budgets](models-grading-and-budgets.md) | OpenAI-compatible and Anthropic providers (native tool calls included), HTTP and `output_expr`, LLM-rubric grading judged by either vendor, similarity, budgets — and every top-level key at once, annotated. |

| #   | Example | Demonstrates |
| --- | ------- | ------------ |
| 01  | [Hello, eval](first-steps.md#example-01--hello-eval) | The smallest suite that runs. No model, no key, no toolchain. |
| 02  | [Prompts and variables](first-steps.md#example-02--prompts-and-variables) | Prompt templates filled per case — and why a run is a grid. |
| 03  | [Deterministic assertions](first-steps.md#example-03--deterministic-assertions) | Every zero-cost assertion type, on one page. |
| 04  | [Structured output](first-steps.md#example-04--structured-output) | `is-json` versus `contains-json` with a schema. |
| 05  | [Weights and thresholds](first-steps.md#example-05--weights-and-thresholds) | How a case decides pass or fail, and how to give partial credit. |
| 06  | [The `!raw` escape hatch](templates-and-test-data.md#example-06--the-raw-escape-hatch) | Test input that must reach the system byte-for-byte. |
| 07  | [File-content vars](templates-and-test-data.md#example-07--file-content-vars) | Pull a var's value from a file beside the suite — parsed, raw, or sandboxed. |
| 08  | [Matrix sweeps](templates-and-test-data.md#example-08--matrix-sweeps) | Fan one case out over the cartesian product of its axes. |
| 09  | [Datasets from files](templates-and-test-data.md#example-09--datasets-from-files) | Cases in `file://` globs, owned and reviewed separately. |
| 10  | [A CSV dataset](templates-and-test-data.md#example-10--a-csv-dataset) | The format a non-engineer will hand you, read directly. |
| 11  | [Test generators](templates-and-test-data.md#example-11--test-generators) | Cases computed by a program, so coverage cannot drift. |
| 12  | [Render health](your-own-system.md#example-12--render-health) | Grade an external system with zero-LLM assertions. |
| 13  | [Your own system](your-own-system.md#example-13--your-own-system) | The `exec` provider: test what actually ships. |
| 14  | [A custom assertion](your-own-system.md#example-14--a-custom-assertion) | A correctness rule only you can express. |
| 15  | [Tool-call assertions](your-own-system.md#example-15--tool-call-assertions) | Grade the decision, not the prose. |
| 16  | [Tags and filters](running-and-reporting.md#example-16--tags-and-filters) | Running part of a suite. |
| 17  | [Composition](running-and-reporting.md#example-17--composition) | `extends`, `imports`, and how the merge actually works. |
| 18  | [A failing gate](running-and-reporting.md#example-18--a-failing-gate) | What red looks like. Exits 1 on purpose. |
| 19  | [Errors and retries](running-and-reporting.md#example-19--errors-and-retries) | Errors are not failures. Exits 3 on purpose. |
| 20  | [Runner tuning](running-and-reporting.md#example-20--runner-tuning) | Concurrency, rate limits, timeouts. |
| 21  | [Caching](caching-and-statistics.md#example-21--caching) | Not paying twice for the same answer. |
| 22  | [Cache salts](caching-and-statistics.md#example-22--cache-salts) | Busting the cache at the right granularity. |
| 23  | [Repeat and confidence](caching-and-statistics.md#example-23--repeat-and-confidence) | A pass rate with an error bar. |
| 24  | [Baselines and diff](caching-and-statistics.md#example-24--baselines-and-diff) | Gate on regressions, not on an absolute score. |
| 25  | [Output formats](running-and-reporting.md#example-25--output-formats) | One run feeding a human and a machine. |
| 26  | [An OpenAI-compatible endpoint](models-grading-and-budgets.md#example-26--an-openai-compatible-endpoint) | The lingua franca: OpenAI, Ollama, vLLM, LiteLLM, a gateway. |
| 27  | [Anthropic, and what it costs](models-grading-and-budgets.md#example-27--anthropic-and-what-it-costs) | `pricing`, and why a `cost:` budget can be green and enforce nothing. |
| 28  | [A service you already run](models-grading-and-budgets.md#example-28--a-service-you-already-run) | The `http` provider and `output_expr`. |
| 29  | [LLM-rubric grading](models-grading-and-budgets.md#example-29--llm-rubric-grading) | A structured, fail-closed grader — and how to write its rubric. |
| 30  | [Similarity](models-grading-and-budgets.md#example-30--similarity) | Cosine distance, for when many wordings are right. |
| 31  | [Budgets](models-grading-and-budgets.md#example-31--budgets) | Cost, tokens, latency — and how each can enforce nothing. |
| 32  | [A live endpoint](models-grading-and-budgets.md#example-32--a-live-endpoint) | Point a suite at your own OpenAI-compatible endpoint. |
| 33  | [An OpenAI-shaped grader](models-grading-and-budgets.md#example-33--an-openai-shaped-grader) | An `llm-rubric` grader that is `type: openai` — any compatible endpoint, including a local Ollama. |
| 34  | [A multi-turn conversation](templates-and-test-data.md#example-34--a-multi-turn-conversation) | A `messages:` prompt carrying real history, not just the newest line. |
| 35  | [Anthropic tools, natively](models-grading-and-budgets.md#example-35--anthropic-tools-natively) | Tool-call grading over the native API — the sibling of example 15's `exec` version. |
| 36  | [`output_expr`, sliced two ways](models-grading-and-budgets.md#example-36--output_expr-sliced-two-ways) | Pulling more than one shape — including a non-string one — out of the same response. |
| 37  | [The exec protocol, in bash](your-own-system.md#example-37--the-exec-protocol-in-bash) | The same provider contract, spoken in bash and `jq` instead of Python. |
| 38  | [Every key, once](models-grading-and-budgets.md#example-38--every-key-once) | One runnable suite setting every top-level key, annotated as a map of the reference. |
| 39  | [A promptfoo config, converted](running-and-reporting.md#example-39--a-promptfoo-config-converted) | A promptfoo config and the suite `domarinn import promptfoo` turns it into, both shipped. |
| 40  | [A rubric that sees the tool calls](models-grading-and-budgets.md#example-40--a-rubric-that-sees-the-tool-calls) | `include_tool_calls`, for grading the delegation decision instead of the prose. |

## See also

- [Install](../start/install.md) / [Your first eval](../start/first-eval.md) — install, then write and run your first suite.
- [domarinn.yaml](../reference/domarinn-yaml.md) — the complete `domarinn.yaml` reference.
- [Assertions](../reference/assertions.md) — every assertion type, weights, thresholds, and short-circuiting.
- [Providers](../reference/providers.md) — `exec`, `http`, `anthropic`, `openai`, and `embeddings`.

<!-- The monolithic examples page lived at this URL, so links published before
     the split carry fragments of the form "#example-13" plus the heading slug.
     The split moved every such heading onto a group page, and this page
     reoccupies the old URL, so mkdocs-redirects cannot forward them (there is
     no old path to stub). Forward the fragment instead: the table above has exactly one
     row link per example anchor, kept complete and pointing at the page that
     really transcludes each example by a guard in
     crates/domarinn-cli/tests/examples.rs. -->
<script>
  (function () {
    var hash = location.hash;
    if (!/^#example-\d\d(-|$)/.test(hash)) {
      return;
    }
    var row =
      document.querySelector('a[href*="' + hash + '"]') ||
      document.querySelector('a[href*="' + hash.slice(0, 11) + '-"]');
    if (row) {
      location.replace(row.href);
    }
  })();
</script>
