//! The shipped examples, and the evidence that each one works.
//!
//! Every directory under `examples/` must appear here exactly once — enforced
//! by `every_shipped_example_is_in_the_table`. That guard is what makes "no
//! example ships untested" a property of the repository rather than of whoever
//! reviewed the last pull request. Adding an example is adding a row; there is
//! no other step.
//!
//! A row is a claim about the *shipped bytes*. The harness runs the real
//! `domarinn` binary against `examples/<dir>/domarinn.yaml` in place, with
//! nothing copied and nothing rewritten — the same file the documentation
//! transcludes. Endpoints move by environment only ([`Env::StubBase`]), which
//! is a capability the example's own YAML declares with `${env:…}`.

#![allow(dead_code)]

use crate::spec::{
    Cells, Example, Step, REPEAT_3, RUN, RUN_AGAIN, RUN_AGAINST_LATEST, RUN_JUNIT, RUN_WITH_SUMMARY,
};

// `#[path]` for the same reason `examples.rs` needs it: a module loaded by path
// resolves its children against the *containing* directory, so a bare
// `mod rows_late;` here would look for `tests/examples/rows_late.rs`.
#[path = "table/rows_late.rs"]
mod rows_late;

use std::sync::LazyLock;

/// Every shipped example, in ladder order.
///
/// A `LazyLock<Vec<_>>` rather than a `const` slice because the rows outgrew
/// the 1000-line file cap and Rust cannot concatenate `const` slices. The join
/// happens once, on first use; the rows themselves are still `const` data.
pub static EXAMPLES: LazyLock<Vec<&'static Example>> =
    LazyLock::new(|| ROWS_EARLY.iter().chain(rows_late::ROWS).collect());

/// Examples 01–25: the ladder from a first eval up to caching and output
/// formats. The later half lives in [`rows_late`] — see its module doc for why
/// the seam is here and which file a new example goes in.
const ROWS_EARLY: &[Example] = &[
    Example {
        dir: "01-hello-eval",
        shows: "the smallest suite that runs: one exec provider, one assertion",
        env: &[],
        stub: &[],
        stub_calls: 0,
        steps: &[Step {
            argv: RUN,
            exit: 0,
            cells: Cells::pass(1),
            case_ids: &["greeting/basic"],
            priced: false,
            writes: &[],
            cache_hits: 0,
        }],
    },
    Example {
        dir: "02-prompts-and-vars",
        shows: "prompt templates filled by per-case vars, over a prompt x test grid",
        env: &[],
        stub: &[],
        stub_calls: 0,
        steps: &[Step {
            argv: RUN,
            exit: 0,
            // Two prompts x three cases. The *cell* count is 6; the distinct
            // test ids below are 3, because a test id names the case, not the
            // cell. Asserting both is what proves the grid actually fanned out
            // rather than one prompt silently winning.
            cells: Cells::pass(6),
            case_ids: &["refund/policy", "export/format", "refund/other-product"],
            priced: false,
            writes: &[],
            cache_hits: 0,
        }],
    },
    Example {
        dir: "03-deterministic-asserts",
        shows: "every zero-cost assertion type on one page",
        env: &[],
        stub: &[],
        stub_calls: 0,
        steps: &[Step {
            argv: RUN,
            exit: 0,
            cells: Cells::pass(5),
            case_ids: &[
                "substring/exact-case",
                "shape/prefix-and-pattern",
                "equality/verbatim",
                "size/within-budget",
                "expression/jinja",
            ],
            priced: false,
            writes: &[],
            cache_hits: 0,
        }],
    },
    Example {
        dir: "04-json-output",
        shows: "`is-json` versus `contains-json` with a schema",
        env: &[],
        stub: &[],
        stub_calls: 0,
        steps: &[Step {
            argv: RUN,
            exit: 0,
            cells: Cells::pass(3),
            case_ids: &[
                "extract/clean",
                "extract/wrapped-in-prose",
                "extract/any-json",
            ],
            priced: false,
            writes: &[],
            cache_hits: 0,
        }],
    },
    Example {
        dir: "05-weights-and-thresholds",
        shows: "how a case decides pass or fail: weighted mean against a threshold",
        env: &[],
        stub: &[],
        stub_calls: 0,
        steps: &[Step {
            argv: RUN,
            exit: 0,
            // Every case passes, but two of them do so with a *failing*
            // assertion inside — which is the whole subject of the page. The
            // scores those cases must reach (0.667 and 0.75) are asserted by
            // `example_05_scores_are_what_its_comments_claim` in examples.rs;
            // a green tally here would not notice if the arithmetic changed.
            cells: Cells::pass(4),
            case_ids: &[
                "gate/all-must-pass",
                "gate/partial-credit",
                "gate/weighted",
                "gate/must-not-leak",
            ],
            priced: false,
            writes: &[],
            cache_hits: 0,
        }],
    },
    Example {
        dir: "06-raw-escape-hatch",
        shows: "keeping literal template syntax literal with `!raw` / `{$raw:}`",
        env: &[],
        stub: &[],
        stub_calls: 0,
        steps: &[Step {
            argv: RUN,
            exit: 0,
            cells: Cells::pass(4),
            case_ids: &[
                "injection/ssti-literal",
                "injection/ssti-object-form",
                "injection/raw-expectation",
                "injection/control-is-rendered",
            ],
            priced: false,
            writes: &[],
            cache_hits: 0,
        }],
    },
    Example {
        dir: "07-file-vars",
        shows: "`!file` fixtures resolved beside the suite, parsed or raw",
        env: &[],
        stub: &[],
        stub_calls: 0,
        steps: &[Step {
            argv: RUN,
            exit: 0,
            cells: Cells::pass(4),
            case_ids: &[
                "doc/echo",
                "doc/structured-rubric",
                "doc/raw-json-text",
                "adversarial/ssti-fixture",
            ],
            priced: false,
            writes: &[],
            cache_hits: 0,
        }],
    },
    Example {
        dir: "08-matrix-sweeps",
        shows: "a case fanning out over the cartesian product of its axes",
        env: &[],
        stub: &[],
        stub_calls: 0,
        steps: &[Step {
            argv: RUN,
            exit: 0,
            // 2 styles x 2 temperatures, plus a 3-value locale sweep.
            cells: Cells::pass(7),
            case_ids: &[
                "greet[style=terse,temperature=0]",
                "greet[style=terse,temperature=1]",
                "greet[style=warm,temperature=0]",
                "greet[style=warm,temperature=1]",
                "sweep-en",
                "sweep-fr",
                "sweep-de",
            ],
            priced: false,
            writes: &[],
            cache_hits: 0,
        }],
    },
    Example {
        dir: "09-dataset-glob",
        shows: "cases loaded from `file://` globs instead of written inline",
        env: &[],
        stub: &[],
        stub_calls: 0,
        steps: &[Step {
            argv: RUN,
            exit: 0,
            cells: Cells::pass(3),
            case_ids: &[
                "refunds/approved",
                "refunds/declined-with-reason",
                "tone/no-blame",
            ],
            priced: false,
            writes: &[],
            cache_hits: 0,
        }],
    },
    Example {
        dir: "10-dataset-csv",
        shows: "a CSV dataset, with the reserved `id`/`tags`/`__assert` columns",
        env: &[],
        stub: &[],
        stub_calls: 0,
        steps: &[Step {
            argv: RUN,
            exit: 0,
            cells: Cells::pass(3),
            case_ids: &["intent/refund", "intent/cancel", "intent/praise"],
            priced: false,
            writes: &[],
            cache_hits: 0,
        }],
    },
    Example {
        dir: "11-test-generators",
        shows: "cases computed by a program, so coverage cannot drift",
        env: &[],
        stub: &[],
        stub_calls: 0,
        steps: &[Step {
            argv: RUN,
            exit: 0,
            // Two banned phrases + two locales, straight from the suite's
            // `config`. Asserting the ids proves the generator actually read
            // that config rather than emitting something hardcoded.
            cells: Cells::pass(4),
            case_ids: &[
                "banned/as-an-ai",
                "banned/i-cannot-help",
                "locale/en",
                "locale/de",
            ],
            priced: false,
            writes: &[],
            cache_hits: 0,
        }],
    },
    Example {
        dir: "12-render-health",
        shows: "grading an external system with zero-LLM assertions, and `!raw`",
        env: &[],
        stub: &[],
        stub_calls: 0,
        steps: &[Step {
            argv: RUN,
            exit: 0,
            cells: Cells::pass(2),
            case_ids: &["adversarial/ssti-literal", "greeting/basic"],
            priced: false,
            writes: &[],
            cache_hits: 0,
        }],
    },
    Example {
        dir: "13-exec-provider",
        shows: "your own program as the system under test, and two of them compared",
        env: &[],
        stub: &[],
        stub_calls: 0,
        steps: &[Step {
            argv: RUN,
            exit: 0,
            // Two providers x two cases. A tally of 2 would mean the second
            // provider silently stopped running.
            cells: Cells::pass(4),
            case_ids: &["billing/refund-window", "billing/escalation"],
            priced: false,
            writes: &[],
            cache_hits: 0,
        }],
    },
    Example {
        dir: "14-custom-exec-assert",
        shows: "an assertion you write yourself, over the exec protocol",
        env: &[],
        stub: &[],
        stub_calls: 0,
        steps: &[Step {
            argv: RUN,
            exit: 0,
            cells: Cells::pass(2),
            case_ids: &["invoice/adds-up", "invoice/rounding-slack"],
            priced: false,
            writes: &[],
            cache_hits: 0,
        }],
    },
    Example {
        dir: "15-tool-call-asserts",
        shows: "grading the decision — which tool, with which arguments",
        env: &[],
        stub: &[],
        stub_calls: 0,
        steps: &[Step {
            argv: RUN,
            exit: 0,
            cells: Cells::pass(3),
            case_ids: &[
                "agent/looks-up-first",
                "agent/refund-shape",
                "agent/no-tool-needed",
            ],
            priced: false,
            writes: &[],
            cache_hits: 0,
        }],
    },
    Example {
        dir: "16-tags-and-filters",
        shows: "running part of a suite: tags, id globs, and per-case provider lists",
        env: &[],
        stub: &[],
        stub_calls: 0,
        steps: &[Step {
            argv: RUN,
            exit: 0,
            // Two providers x four cases is eight, minus the one case pinned to
            // a single provider by `only_providers`. Seven is the whole point
            // of the example; eight would mean the pin stopped working.
            cells: Cells::pass(7),
            case_ids: &[
                "billing/refund",
                "billing/invoice",
                "safety/no-credentials",
                "safety/cites-policy",
            ],
            priced: false,
            writes: &[],
            cache_hits: 0,
        }],
    },
    Example {
        dir: "17-defaults-and-composition",
        shows: "`extends` and `defaults`: maps merge, sequences replace, `assert` appends",
        env: &[],
        stub: &[],
        stub_calls: 0,
        steps: &[Step {
            argv: RUN,
            exit: 0,
            cells: Cells::pass(2),
            case_ids: &["support/mentions-product", "support/stays-in-character"],
            priced: false,
            writes: &[],
            cache_hits: 0,
        }],
    },
    Example {
        dir: "18-failing-gate",
        shows: "what red looks like: exit 1, a mixed tally, and short-circuiting",
        env: &[],
        stub: &[],
        stub_calls: 0,
        steps: &[Step {
            argv: RUN,
            // The page's entire subject. A row asserting 0 here would let the
            // example go quietly green and still read as a failure demo.
            exit: 1,
            cells: Cells {
                passed: 1,
                failed: 2,
                errored: 0,
                skipped: 0,
            },
            case_ids: &[
                "refusal/stays-in-scope",
                "privacy/no-internal-ids",
                "tone/apologises",
            ],
            priced: false,
            writes: &[],
            cache_hits: 0,
        }],
    },
    Example {
        dir: "19-errors-and-retries",
        shows: "errors are not failures: exit 3, retriable vs fatal, empty-reason skips",
        env: &[],
        stub: &[],
        stub_calls: 0,
        steps: &[Step {
            argv: RUN,
            // 3 beats 1 so CI can tell "the model got worse" from "the harness
            // broke". This suite contains no failing assertion at all, so a 1
            // here would mean the two had been conflated.
            exit: 3,
            cells: Cells {
                passed: 1,
                failed: 0,
                errored: 2,
                skipped: 1,
            },
            case_ids: &[
                "ok/plain-answer",
                "empty/refusal-is-skipped",
                "error/retriable-gives-up",
                "error/fatal-not-retried",
            ],
            priced: false,
            writes: &[],
            cache_hits: 0,
        }],
    },
    Example {
        dir: "20-runner-tuning",
        shows: "concurrency, rate limiting, timeouts, and short-circuiting",
        env: &[],
        stub: &[],
        stub_calls: 0,
        steps: &[Step {
            argv: RUN,
            exit: 0,
            cells: Cells::pass(4),
            case_ids: &[
                "throughput/a",
                "throughput/b",
                "throughput/c",
                "throughput/d",
            ],
            priced: false,
            writes: &[],
            cache_hits: 0,
        }],
    },
    Example {
        dir: "21-caching-basics",
        shows: "a second run of an unchanged suite answers entirely from cache",
        env: &[],
        stub: &[],
        stub_calls: 0,
        steps: &[
            Step {
                argv: RUN,
                exit: 0,
                cells: Cells::pass(3),
                case_ids: &["summary/short", "summary/long", "summary/empty-ish"],
                priced: false,
                // A cold run against a fresh --cache-dir. Asserting 0 here is
                // what makes the 3 below mean something.
                writes: &[],
                cache_hits: 0,
            },
            Step {
                argv: RUN_AGAIN,
                exit: 0,
                cells: Cells::pass(3),
                case_ids: &["summary/short", "summary/long", "summary/empty-ish"],
                priced: false,
                // The claim the page makes, asserted: every cell served warm.
                writes: &[],
                cache_hits: 3,
            },
        ],
    },
    Example {
        dir: "22-cache-salts",
        shows: "busting the cache at the right granularity: a build pin plus `$digest:`",
        env: &[],
        stub: &[],
        stub_calls: 0,
        steps: &[Step {
            argv: RUN,
            exit: 0,
            cells: Cells::pass(3),
            case_ids: &[
                "prompts/greeting",
                "prompts/escalation",
                "prompts/whole-flow",
            ],
            priced: false,
            writes: &[],
            cache_hits: 0,
        }],
    },
    Example {
        dir: "23-repeat-and-confidence",
        shows: "`--repeat` re-samples every cell, so a pass rate gets an error bar",
        env: &[],
        stub: &[],
        stub_calls: 0,
        steps: &[Step {
            argv: REPEAT_3,
            exit: 0,
            // 2 cases x 3 trials. A tally of 2 would mean `--repeat` silently
            // stopped fanning out, which is the whole subject of the page.
            cells: Cells::pass(6),
            case_ids: &["stability/greeting", "stability/refusal"],
            priced: false,
            writes: &[],
            cache_hits: 0,
        }],
    },
    Example {
        dir: "24-baselines-and-diff",
        shows: "gating on regressions against a baseline, not on an absolute score",
        env: &[],
        stub: &[],
        stub_calls: 0,
        steps: &[
            Step {
                argv: RUN,
                exit: 0,
                cells: Cells::pass(2),
                case_ids: &["regression/policy-window", "regression/no-blame"],
                priced: false,
                writes: &[],
                cache_hits: 0,
            },
            // The second run resolves `--against latest` out of the run store
            // the first one wrote. Both steps share a cwd, which is the only
            // reason `latest` finds anything — exactly the condition a fresh CI
            // checkout does NOT satisfy, which is the trap the page documents.
            Step {
                argv: RUN_AGAINST_LATEST,
                exit: 0,
                cells: Cells::pass(2),
                case_ids: &["regression/policy-window", "regression/no-blame"],
                priced: false,
                writes: &[],
                cache_hits: 0,
            },
        ],
    },
    Example {
        dir: "25-output-formats",
        shows: "one run feeding a human and a machine at once",
        env: &[],
        stub: &[],
        stub_calls: 0,
        steps: &[
            Step {
                argv: RUN_WITH_SUMMARY,
                exit: 0,
                cells: Cells::pass(2),
                case_ids: &["report/passing-case", "report/second-case"],
                priced: false,
                // The cell tallies say nothing about the reporters: a green run
                // whose summary writer produced an empty file looks identical.
                writes: &["{tmp}/summary.md"],
                cache_hits: 0,
            },
            Step {
                argv: RUN_JUNIT,
                exit: 0,
                // No JSON result document this time — the JUnit report IS the
                // output under test, and it is checked by `writes`.
                cells: Cells::NONE,
                case_ids: &[],
                priced: false,
                writes: &["{tmp}/results.xml"],
                cache_hits: 0,
            },
        ],
    },
];
