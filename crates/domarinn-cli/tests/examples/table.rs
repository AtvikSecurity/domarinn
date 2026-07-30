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

use crate::stubs;

/// One shipped example and everything needed to prove it works.
pub struct Example {
    /// Directory name under `examples/`. Also the path the docs transclude.
    pub dir: &'static str,
    /// One line: what the docs use this example to show. Printed on failure, so
    /// the operator learns the *point* of the example without opening it.
    pub shows: &'static str,
    /// Environment for the child process, over and above the scrubbed baseline.
    /// The only lever that redirects a networked example at the stub.
    pub env: &'static [(&'static str, Env)],
    /// Endpoints the stub answers, keyed by a fragment of the request line.
    ///
    /// Each route scripts a *sequence* of bodies, one per matching request. A
    /// single body cannot express a case whose point is that two calls differ —
    /// `similar` embeds the output and the reference, so one repeated embedding
    /// scores 1.0 against itself and the threshold goes untested. Runs are
    /// serial by default, so the order is stable.
    pub stub: &'static [Route],
    /// Exactly how many requests the stub must serve across every step.
    ///
    /// Not decoration — this is the egress guard. If `${env:…}` ever stopped
    /// redirecting `base_url`, the request would go to the real vendor and the
    /// stub would serve zero, so a count mismatch is how "this test quietly
    /// started calling api.anthropic.com" gets caught. `0` asserts the example
    /// is genuinely offline.
    pub stub_calls: usize,
    /// The invocations, in order. More than one for an example whose point is a
    /// second run — a warm cache, a baseline diff.
    pub steps: &'static [Step],
}

/// One `domarinn` invocation and what it must produce.
pub struct Step {
    /// Everything after `domarinn`. `{dir}` is the example's absolute path and
    /// `{tmp}` the per-row scratch directory. Written out in full rather than
    /// assembled from flags, so a failure can print a command that pastes into
    /// a shell unchanged.
    pub argv: &'static [&'static str],
    pub exit: u8,
    /// The tallies this step's result document must carry. [`Cells::NONE`] for
    /// a step that writes none (`validate`, `list`).
    pub cells: Cells,
    /// Every `cell.test_id` the run must contain, order-insensitive. Empty only
    /// when `cells` is [`Cells::NONE`].
    pub case_ids: &'static [&'static str],
    /// Assert the run actually priced itself.
    ///
    /// `AssertKind::Cost` *passes* when `cost_usd` is `None` — "cost not
    /// reported; budget not enforced". So an example whose page claims a budget
    /// was enforced can be green while enforcing nothing, if the stub forgot
    /// `usage` or the model is not in the pricing table. Set this on any such
    /// example.
    pub priced: bool,
    /// Files this step must have written, non-empty, when it finishes.
    ///
    /// `{tmp}` is substituted as in `argv`. Exists for the examples whose whole
    /// subject is a side file — a JUnit report, a Markdown summary — which the
    /// cell tallies say nothing about: a run can be perfectly green while the
    /// reporter it is demonstrating writes nothing at all.
    pub writes: &'static [&'static str],
    /// The run must report at least this many cache hits.
    ///
    /// `0` for a step that should do real work. A caching example's whole claim
    /// is that its *second* run pays nothing, and without this the second step
    /// would look identical to the first whether the cache worked or not.
    pub cache_hits: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Cells {
    pub passed: u64,
    pub failed: u64,
    pub errored: u64,
    pub skipped: u64,
}

impl Cells {
    /// A step that writes no result document.
    pub const NONE: Cells = Cells {
        passed: 0,
        failed: 0,
        errored: 0,
        skipped: 0,
    };

    /// The common case: everything green.
    pub const fn pass(n: u64) -> Cells {
        Cells {
            passed: n,
            failed: 0,
            errored: 0,
            skipped: 0,
        }
    }

    pub fn total(&self) -> u64 {
        self.passed + self.failed + self.errored + self.skipped
    }
}

/// A stubbed endpoint: matched with `request_line.contains(fragment)`.
pub struct Route {
    pub fragment: &'static str,
    /// One body per matching request; the last repeats once exhausted.
    pub bodies: &'static [&'static str],
}

/// A value for the child's environment.
pub enum Env {
    Literal(&'static str),
    /// `http://127.0.0.1:<port>` — an Anthropic `base_url`, which the client
    /// suffixes with `/v1/messages`.
    StubBase,
    /// `http://127.0.0.1:<port>/v1` — an OpenAI-compatible `base_url`, which
    /// the client suffixes with `/chat/completions` (its real default already
    /// ends in `/v1`). Carried so an example's default value stays byte-honest
    /// about the endpoint shape.
    StubBaseV1,
}

/// The standard run: JSON to a scratch file, no progress bar, isolated cache.
///
/// `--cache-dir` is not optional. The cache defaults to
/// `<suite dir>/.domarinn/cache`, so without it a run writes into the
/// repository — and `.gitignore` hides it, which makes it worse: the *next*
/// run of that example is then served from a stale cache, the stub sees zero
/// requests, and the egress guard fails for a reason that looks like a broken
/// example.
pub const RUN: &[&str] = &[
    "run",
    "{dir}",
    "--format",
    "json",
    "--out",
    "{tmp}/result.json",
    "--no-progress",
    "--cache-dir",
    "{tmp}/cache",
];

/// A second run against the same cache directory — the warm half of a caching
/// example.
pub const RUN_AGAIN: &[&str] = RUN;

/// [`RUN`] with three trials per cell, for the confidence-interval example.
pub const REPEAT_3: &[&str] = &[
    "run",
    "{dir}",
    "--format",
    "json",
    "--out",
    "{tmp}/result.json",
    "--no-progress",
    "--cache-dir",
    "{tmp}/cache",
    "--repeat",
    "3",
];

/// [`RUN`] gated against the previous run in the local store.
pub const RUN_AGAINST_LATEST: &[&str] = &[
    "run",
    "{dir}",
    "--format",
    "json",
    "--out",
    "{tmp}/result.json",
    "--no-progress",
    "--cache-dir",
    "{tmp}/cache",
    "--against",
    "latest",
];

/// [`RUN`] plus the Markdown summary CI pastes into a job summary.
///
/// `--out` takes a single path, so a run emits ONE machine format to a file.
/// Producing a second one is a second invocation — which is what the row does,
/// and what the page shows.
pub const RUN_WITH_SUMMARY: &[&str] = &[
    "run",
    "{dir}",
    "--format",
    "json",
    "--out",
    "{tmp}/result.json",
    "--no-progress",
    "--cache-dir",
    "{tmp}/cache",
    "--summary-md",
    "{tmp}/summary.md",
];

/// The same suite again, emitting JUnit for a CI system to render.
pub const RUN_JUNIT: &[&str] = &[
    "run",
    "{dir}",
    "--no-progress",
    "--cache-dir",
    "{tmp}/cache",
    "--format",
    "junit",
    "--out",
    "{tmp}/results.xml",
];

/// Parse and structurally validate only. For an example that cannot run here
/// because it deliberately points at an endpoint only its reader has.
pub const VALIDATE: &[&str] = &["validate", "{dir}"];

pub const EXAMPLES: &[Example] = &[
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
    Example {
        dir: "26-openai-provider",
        shows: "an OpenAI-compatible endpoint, and where the key does and does not go",
        env: &[
            // The example's own `${env:OPENAI_BASE_URL:-…}` is what makes this
            // possible. If that line were ever removed, the request would go to
            // api.openai.com and `stub_calls` below would see 0.
            ("OPENAI_BASE_URL", Env::StubBaseV1),
            ("OPENAI_API_KEY", Env::Literal("sk-stub-not-a-real-key")),
        ],
        stub: &[Route {
            fragment: "/chat/completions",
            // Two DIFFERENT bodies: one repeated answer would satisfy both
            // cases only by coincidence, and would still pass if the vars
            // stopped reaching the prompt.
            bodies: &[stubs::OPENAI_TEXT, stubs::OPENAI_TEXT_ALT],
        }],
        stub_calls: 2,
        steps: &[Step {
            argv: RUN,
            exit: 0,
            cells: Cells::pass(2),
            case_ids: &["capital/france", "capital/norway"],
            priced: false,
            writes: &[],
            cache_hits: 0,
        }],
    },
    Example {
        dir: "27-anthropic-provider",
        shows: "Anthropic, plus a `pricing` block that makes a cost budget real",
        env: &[
            ("ANTHROPIC_BASE_URL", Env::StubBase),
            (
                "ANTHROPIC_API_KEY",
                Env::Literal("sk-ant-stub-not-a-real-key"),
            ),
        ],
        stub: &[Route {
            fragment: "/v1/messages",
            bodies: &[stubs::ANTHROPIC_TEXT],
        }],
        stub_calls: 1,
        steps: &[Step {
            argv: RUN,
            exit: 0,
            cells: Cells::pass(1),
            case_ids: &["policy/return-window"],
            // The page claims its `cost:` budget is enforced. Without this the
            // example could ship with the `pricing` block deleted, price at
            // nothing, and the cost assertion would still pass — as "cost not
            // reported".
            priced: true,
            writes: &[],
            cache_hits: 0,
        }],
    },
    Example {
        dir: "28-http-provider",
        shows: "a service you already run, with `output_expr` selecting the answer",
        env: &[
            ("SUPPORT_API_URL", Env::StubBase),
            ("SUPPORT_API_TOKEN", Env::Literal("stub-token")),
        ],
        stub: &[Route {
            // The example posts to the stub root, so match the method rather
            // than a vendor path — an `http` provider has no fixed one.
            fragment: "POST /",
            bodies: &[stubs::SERVICE_REPLY],
        }],
        stub_calls: 1,
        steps: &[Step {
            argv: RUN,
            exit: 0,
            cells: Cells::pass(1),
            case_ids: &["orders/status"],
            priced: false,
            writes: &[],
            cache_hits: 0,
        }],
    },
    Example {
        dir: "29-llm-rubric-grading",
        shows: "a structured, fail-closed LLM judge, and how to write its rubric",
        env: &[
            ("ANTHROPIC_BASE_URL", Env::StubBase),
            (
                "ANTHROPIC_API_KEY",
                Env::Literal("sk-ant-stub-not-a-real-key"),
            ),
        ],
        stub: &[Route {
            fragment: "/v1/messages",
            bodies: &[stubs::ANTHROPIC_VERDICT_PASS],
        }],
        // One grader call: the system under test is an offline exec provider,
        // so the only thing reaching the network is the judge.
        stub_calls: 1,
        steps: &[Step {
            argv: RUN,
            exit: 0,
            cells: Cells::pass(1),
            case_ids: &["refusal/declines-and-redirects"],
            priced: false,
            writes: &[],
            cache_hits: 0,
        }],
    },
    Example {
        dir: "30-similar-embeddings",
        shows: "cosine similarity to a reference, when many wordings are right",
        env: &[
            ("OPENAI_BASE_URL", Env::StubBaseV1),
            ("OPENAI_API_KEY", Env::Literal("sk-stub-not-a-real-key")),
        ],
        stub: &[Route {
            fragment: "/embeddings",
            // Two DIFFERENT vectors, in call order — the output then the
            // reference. One repeated body would give cosine 1.0 against
            // itself, and the 0.85 threshold on the page would be untested at
            // every value.
            bodies: &[stubs::EMBED_A, stubs::EMBED_NEAR_A],
        }],
        stub_calls: 2,
        steps: &[Step {
            argv: RUN,
            exit: 0,
            cells: Cells::pass(1),
            case_ids: &["paraphrase/policy-window"],
            priced: false,
            writes: &[],
            cache_hits: 0,
        }],
    },
    Example {
        dir: "31-budgets",
        shows: "cost, token and latency budgets — and how each can enforce nothing",
        env: &[],
        stub: &[],
        stub_calls: 0,
        steps: &[Step {
            argv: RUN,
            exit: 0,
            cells: Cells::pass(2),
            case_ids: &["budget/cheap-answer", "budget/cache-writes-are-billable"],
            // The page's whole warning is that a `cost:` budget passes as "cost
            // not reported" when nothing priced the call. If the provider ever
            // stopped reporting cost_usd, both cases would stay green and the
            // example would teach the opposite of what it says.
            priced: true,
            writes: &[],
            cache_hits: 0,
        }],
    },
    Example {
        dir: "32-live-endpoint-smoke",
        shows: "pointing a suite at your own OpenAI-compatible endpoint",
        // Deliberately validate-only. Every `${env:…}` in this suite is
        // *without* a default, so it names an endpoint, model and key that only
        // its reader has — that is the example's whole subject. Running it here
        // would mean inventing values, which would document a suite nobody
        // could reproduce. `validate` still proves the file parses, that its
        // interpolation is well-formed, and that its schema reference is live.
        env: &[
            (
                "DOMARINN_SMOKE_BASE_URL",
                Env::Literal("https://example.invalid/v1"),
            ),
            ("DOMARINN_SMOKE_MODEL", Env::Literal("a-model")),
            ("DOMARINN_SMOKE_API_KEY", Env::Literal("unused-by-validate")),
        ],
        stub: &[],
        stub_calls: 0,
        steps: &[Step {
            argv: VALIDATE,
            exit: 0,
            cells: Cells::NONE,
            case_ids: &[],
            priced: false,
            writes: &[],
            cache_hits: 0,
        }],
    },
    Example {
        dir: "33-openai-grader-rubric",
        shows: "an llm-rubric judge that is OpenAI-shaped instead of Anthropic — any \
                OpenAI-compatible endpoint, local or hosted, can grade",
        env: &[
            ("OPENAI_BASE_URL", Env::StubBaseV1),
            ("OPENAI_API_KEY", Env::Literal("sk-stub-not-a-real-key")),
        ],
        stub: &[Route {
            fragment: "/chat/completions",
            bodies: &[stubs::OPENAI_VERDICT_PASS],
        }],
        // One grader call: the system under test is an offline exec provider,
        // so the only thing reaching the network is the judge.
        stub_calls: 1,
        steps: &[Step {
            argv: RUN,
            exit: 0,
            cells: Cells::pass(1),
            case_ids: &["policy/no-invented-exceptions"],
            priced: false,
            writes: &[],
            cache_hits: 0,
        }],
    },
    Example {
        dir: "34-multi-turn-conversation",
        shows: "a `messages:` prompt carrying real history, not just the newest line",
        env: &[],
        stub: &[],
        stub_calls: 0,
        steps: &[Step {
            argv: RUN,
            exit: 0,
            cells: Cells::pass(2),
            case_ids: &["turns/asks-about-electronics", "turns/asks-about-receipts"],
            priced: false,
            writes: &[],
            cache_hits: 0,
        }],
    },
    Example {
        dir: "35-anthropic-tools",
        shows: "tool-call grading over the native Anthropic API, not just an exec provider",
        env: &[
            ("ANTHROPIC_BASE_URL", Env::StubBase),
            (
                "ANTHROPIC_API_KEY",
                Env::Literal("sk-ant-stub-not-a-real-key"),
            ),
        ],
        stub: &[Route {
            fragment: "/v1/messages",
            bodies: &[stubs::ANTHROPIC_TOOL_USE],
        }],
        stub_calls: 1,
        steps: &[Step {
            argv: RUN,
            exit: 0,
            cells: Cells::pass(1),
            case_ids: &["agent/looks-up-order"],
            priced: false,
            writes: &[],
            cache_hits: 0,
        }],
    },
    Example {
        dir: "36-http-output-expr",
        shows: "`output_expr` reaching more than one shape out of the same JSON body",
        env: &[("ORDERS_API_URL", Env::StubBase)],
        stub: &[Route {
            // Both providers post to the stub root, same as example 28.
            fragment: "POST /",
            bodies: &[stubs::SERVICE_REPLY],
        }],
        // One call per provider: each test is scoped to a single provider via
        // `only_providers`, so the pair of tests makes exactly two calls.
        stub_calls: 2,
        steps: &[Step {
            argv: RUN,
            exit: 0,
            cells: Cells::pass(2),
            case_ids: &["orders/reply-text", "orders/confidence-score"],
            priced: false,
            writes: &[],
            cache_hits: 0,
        }],
    },
    Example {
        dir: "37-exec-provider-bash",
        shows: "the exec protocol read and answered in bash + jq, not just python",
        env: &[],
        stub: &[],
        stub_calls: 0,
        steps: &[Step {
            argv: RUN,
            exit: 0,
            cells: Cells::pass(1),
            case_ids: &["greeting/echoes-input"],
            priced: false,
            writes: &[],
            cache_hits: 0,
        }],
    },
    Example {
        dir: "38-annotated-reference-suite",
        shows: "every top-level suite key in one file, each annotated with where \
                its full story lives",
        env: &[
            ("OPENAI_BASE_URL", Env::StubBaseV1),
            ("OPENAI_API_KEY", Env::Literal("sk-stub-not-a-real-key")),
        ],
        stub: &[Route {
            fragment: "/chat/completions",
            bodies: &[stubs::OPENAI_VERDICT_PASS],
        }],
        // One grader call, not three: only the inline case carries an
        // `llm-rubric`, and the system under test is the offline echo provider.
        stub_calls: 1,
        steps: &[Step {
            argv: RUN,
            exit: 0,
            // Three cells from two test sources: one inline case plus a
            // two-value matrix axis. `register/plain` is deliberately NOT among
            // them — `plain` is the `defaults.vars` value the inline case
            // inherits, and an axis value overrides it rather than adding a
            // third cell.
            cells: Cells::pass(3),
            case_ids: &[
                "policy/names-the-product",
                "register/casual",
                "register/formal",
            ],
            priced: false,
            writes: &[],
            cache_hits: 0,
        }],
    },
];
