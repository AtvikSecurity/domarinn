//! Examples 26 and up: providers, graders, tools, and the transcript features.
//!
//! Split out of `table.rs` for one reason only — the two halves together are
//! past the 1000-line cap `domarinn-core/tests/file_length.rs` enforces, and
//! Rust has no way to concatenate `const` slices in a `const`. The seam at 26
//! carries no meaning beyond "roughly half"; [`crate::table::EXAMPLES`] joins
//! the two back into one list, and every guard still sees a single table.
//!
//! **A new example goes here**, since the ladder only ever grows at the end.

#![allow(dead_code)]

use crate::spec::{Cells, Env, Example, Route, Step, RUN, VALIDATE};
use crate::stubs;

pub const ROWS: &[Example] = &[
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
    Example {
        dir: "39-import-promptfoo",
        shows: "a promptfoo config and the suite `domarinn import promptfoo` turns \
                it into, both shipped and both exercised",
        env: &[],
        stub: &[],
        stub_calls: 0,
        steps: &[
            // The converter prints to stdout and takes no output path, so this
            // step can only prove it converts the shipped config without
            // erroring. That the printed suite matches the committed one — and
            // runs — is
            // `example_39_the_committed_conversion_is_the_converters_output_and_runs`,
            // which needs the stdout this table cannot capture.
            Step {
                argv: &["import", "promptfoo", "{dir}/promptfooconfig.yaml"],
                exit: 0,
                cells: Cells::NONE,
                case_ids: &[],
                priced: false,
                writes: &[],
                cache_hits: 0,
            },
            // The committed conversion, run in place like every other example:
            // `case-0` / `case-1` are the ids the converter generates for
            // promptfoo cases that carry no description of their own.
            Step {
                argv: RUN,
                exit: 0,
                cells: Cells::pass(2),
                case_ids: &["case-0", "case-1"],
                priced: false,
                writes: &[],
                cache_hits: 0,
            },
        ],
    },
    Example {
        dir: "40-rubric-sees-tool-calls",
        shows: "a rubric shown the tool calls, so the delegation decision is \
                gradeable and not just the prose",
        env: &[
            ("ANTHROPIC_BASE_URL", Env::StubBase),
            (
                "ANTHROPIC_API_KEY",
                Env::Literal("sk-ant-stub-not-a-real-key"),
            ),
        ],
        stub: &[Route {
            fragment: "/v1/messages",
            bodies: &[stubs::ANTHROPIC_VERDICT_TOOL_AWARE],
        }],
        // One grader call, as in example 29: the system under test is an
        // offline exec provider, so the judge is the only thing on the network.
        stub_calls: 1,
        steps: &[Step {
            argv: RUN,
            exit: 0,
            cells: Cells::pass(1),
            case_ids: &["orders/looks-it-up-before-answering"],
            priced: false,
            writes: &[],
            cache_hits: 0,
        }],
    },
    Example {
        dir: "41-per-case-history",
        shows: "each case bringing its own prior turns, spliced at the prompt's \
                `history` marker",
        env: &[],
        stub: &[],
        stub_calls: 0,
        steps: &[Step {
            argv: RUN,
            exit: 0,
            cells: Cells::pass(5),
            case_ids: &[
                "history/first-contact",
                "history/follow-up",
                "history/escalation",
                "csv/with-history",
                "csv/no-history",
            ],
            priced: false,
            writes: &[],
            cache_hits: 0,
        }],
    },
    Example {
        dir: "42-tool-call-history",
        shows: "replaying a tool-using transcript: the call a turn made, the result \
                that came back, and the reasoning behind it",
        env: &[],
        stub: &[],
        stub_calls: 0,
        steps: &[Step {
            argv: RUN,
            exit: 0,
            cells: Cells::pass(5),
            case_ids: &[
                "tools/replays-a-call",
                "tools/parallel-round",
                "tools/templated-arguments",
                "tools/replays-thinking",
                "tools/from-a-file",
            ],
            priced: false,
            writes: &[],
            cache_hits: 0,
        }],
    },
    Example {
        dir: "43-custom-request",
        shows: "`request:` — the auth scheme, headers, query and body overlay a gateway needs",
        env: &[
            ("CLAUDE_GATEWAY_URL", Env::StubBase),
            (
                "CLAUDE_OAUTH_TOKEN",
                Env::Literal("sk-ant-oat01-stub-not-real"),
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
            priced: false,
            writes: &[],
            cache_hits: 0,
        }],
    },
];
