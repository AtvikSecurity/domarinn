//! Unit tests for [`super`] (local assertion evaluation). Split out of
//! `asserts.rs` via `#[path]` to keep that file under the repo's 1000-line
//! source cap; this is still its private child module (`use super::*`).

use super::*;
use serde_json::json;

fn a(kind: AssertKind) -> Assert {
    Assert {
        weight: 1.0,
        negate: false,
        kind,
    }
}

fn eval(assert: &Assert, out: &str) -> AssertOutcome {
    let engine = TemplateEngine::new();
    let vars = json!({});
    let metrics = MetricCtx::default();
    let schemas = crate::jsonschema_cache::SchemaCache::new();
    evaluate_local(
        assert,
        &Output::Text(out.into()),
        &EvalCtx {
            engine: &engine,
            vars: &vars,
            metrics: &metrics,
            schemas: &schemas,
            tool_calls: &[],
            empty_reason: None,
        },
    )
    .unwrap()
}

#[test]
fn contains_and_negate() {
    assert!(
        eval(
            &a(AssertKind::Contains {
                value: "cat".into()
            }),
            "a cat"
        )
        .passed
    );
    let mut neg = a(AssertKind::Contains { value: "49".into() });
    neg.negate = true;
    assert!(
        eval(&neg, "the answer is {{7*7}}").passed,
        "not-contains 49 passes"
    );
    assert!(!eval(&neg, "the answer is 49").passed);
}

#[test]
fn icontains_any() {
    let assert = a(AssertKind::IcontainsAny {
        values: vec!["Cannot".into(), "won't".into()],
    });
    assert!(eval(&assert, "I cannot help with that").passed);
    assert!(!eval(&assert, "sure thing").passed);
}

#[test]
fn regex_and_length_and_starts_with() {
    assert!(
        eval(
            &a(AssertKind::Regex {
                value: r"\d{3}".into()
            }),
            "abc123"
        )
        .passed
    );
    assert!(
        eval(
            &a(AssertKind::Length {
                min: Some(2),
                max: Some(10)
            }),
            "hello"
        )
        .passed
    );
    assert!(
        !eval(
            &a(AssertKind::Length {
                min: None,
                max: Some(3)
            }),
            "toolong"
        )
        .passed
    );
    assert!(eval(&a(AssertKind::StartsWith { value: "he".into() }), "hello").passed);
}

#[test]
fn is_json_and_contains_json() {
    assert!(eval(&a(AssertKind::IsJson), r#"{"a":1}"#).passed);
    assert!(!eval(&a(AssertKind::IsJson), "not json").passed);
    assert!(
        eval(
            &a(AssertKind::ContainsJson { schema: None }),
            "prefix {\"a\":1} suffix"
        )
        .passed
    );
}

#[test]
fn jinja_expression() {
    assert!(
        eval(
            &a(AssertKind::Jinja {
                value: "output | length < 10".into()
            }),
            "short"
        )
        .passed
    );
    assert!(
        !eval(
            &a(AssertKind::Jinja {
                value: "output | length < 3".into()
            }),
            "longer"
        )
        .passed
    );
}

#[test]
fn budget_asserts_use_metrics() {
    let metrics = MetricCtx {
        latency_ms: 500,
        cost_usd: Some(0.01),
        total_tokens: Some(1200),
        billable_tokens: Some(1500),
    };
    let out = Output::Text("x".into());
    let eng = TemplateEngine::new();
    let vars = json!({});
    let schemas = crate::jsonschema_cache::SchemaCache::new();
    let ctx = EvalCtx {
        engine: &eng,
        vars: &vars,
        metrics: &metrics,
        schemas: &schemas,
        tool_calls: &[],
        empty_reason: None,
    };
    let lat = evaluate_local(&a(AssertKind::Latency { max: 1000 }), &out, &ctx).unwrap();
    assert!(lat.passed);
    let toks = evaluate_local(
        &a(AssertKind::Tokens {
            max: 1000,
            count: None,
        }),
        &out,
        &ctx,
    )
    .unwrap();
    assert!(!toks.passed, "1200 tokens exceeds max 1000");
}

#[test]
fn non_local_asserts_return_none() {
    let assert = a(AssertKind::LlmRubric {
        value: "good".into(),
        grader: None,
        threshold: None,
        params: None,
    });
    let engine = TemplateEngine::new();
    let vars = json!({});
    let metrics = MetricCtx::default();
    let schemas = crate::jsonschema_cache::SchemaCache::new();
    assert!(evaluate_local(
        &assert,
        &Output::Text("x".into()),
        &EvalCtx {
            engine: &engine,
            vars: &vars,
            metrics: &metrics,
            schemas: &schemas,
            tool_calls: &[],
            empty_reason: None,
        }
    )
    .is_none());
}

/// Pin test: for every `AssertKind` variant, `AssertKind::name()` must
/// serialize to exactly the same string as the variant's own serde `type`
/// tag. This guards `AssertKind` <-> `AssertName` drift forever — if a
/// variant is added, renamed, or its tag changes without updating
/// `AssertName`/`AssertKind::name`, this test (or a compile error from the
/// exhaustive match in `AssertKind::name`) catches it.
#[test]
fn assert_name_matches_assert_kind_tag_for_every_variant() {
    use crate::val::Val;

    let variants: Vec<AssertKind> = vec![
        AssertKind::Contains {
            value: String::new(),
        },
        AssertKind::Icontains {
            value: String::new(),
        },
        AssertKind::IcontainsAny { values: vec![] },
        AssertKind::Regex {
            value: String::new(),
        },
        AssertKind::Equals {
            value: Val::Raw(Json::Null),
        },
        AssertKind::StartsWith {
            value: String::new(),
        },
        AssertKind::IsJson,
        AssertKind::ContainsJson { schema: None },
        AssertKind::Length {
            min: None,
            max: None,
        },
        AssertKind::Jinja {
            value: String::new(),
        },
        AssertKind::Exec {
            command: vec![],
            config: None,
            cache_salt: None,
        },
        AssertKind::LlmRubric {
            value: String::new(),
            grader: None,
            threshold: None,
            params: None,
        },
        AssertKind::Cost { max: 0.0 },
        AssertKind::Latency { max: 0 },
        AssertKind::Tokens {
            max: 0,
            count: None,
        },
        AssertKind::Similar {
            value: Val::Raw(Json::Null),
            threshold: None,
        },
        AssertKind::ToolCall {
            name: String::new(),
            args: None,
            schema: None,
        },
    ];
    assert_eq!(variants.len(), 17, "update this test when adding a variant");

    for kind in variants {
        // The actual tag the `Assert` config schema produces for this
        // variant (from AssertKind's own `#[serde(tag = "type")]`
        // encoding), not a hand-typed expectation.
        let assert_json = serde_json::to_value(a(kind.clone())).unwrap();
        let tag = assert_json["type"].clone();
        let name_json = serde_json::to_value(kind.name()).unwrap();
        assert_eq!(
            name_json, tag,
            "AssertName for {kind:?} must equal its AssertKind tag"
        );
    }
}

mod contains_json_schema_tests {
    //! `contains-json`'s `schema` field went from parsed-and-ignored to
    //! enforced. These are the regressions that were silently passing.

    use super::*;
    use crate::config::Assert;
    use crate::jsonschema_cache::SchemaCache;
    use crate::val::Val;
    use serde_json::json;

    fn eval_with_schema(output: &str, schema: Option<serde_json::Value>) -> AssertOutcome {
        let engine = TemplateEngine::new();
        let vars = json!({});
        let metrics = MetricCtx::default();
        let schemas = SchemaCache::new();
        evaluate_local(
            &Assert {
                weight: 1.0,
                negate: false,
                kind: AssertKind::ContainsJson {
                    schema: schema.map(Val::Raw),
                },
            },
            &Output::Text(output.into()),
            &EvalCtx {
                engine: &engine,
                vars: &vars,
                metrics: &metrics,
                schemas: &schemas,
                tool_calls: &[],
                empty_reason: None,
            },
        )
        .unwrap()
    }

    /// The regression: this used to pass, because `schema` was `let _ = schema;`.
    #[test]
    fn a_schema_mismatch_now_fails() {
        let schema = json!({"type": "object", "required": ["age"],
                            "properties": {"age": {"type": "integer"}}});
        assert!(!eval_with_schema(r#"here: {"age": "seven"}"#, Some(schema.clone())).passed);
        assert!(!eval_with_schema(r#"{"name": "x"}"#, Some(schema)).passed);
    }

    #[test]
    fn a_matching_document_passes() {
        let schema = json!({"type": "object", "properties": {"age": {"type": "integer"}}});
        assert!(eval_with_schema(r#"result: {"age": 7}"#, Some(schema)).passed);
    }

    /// Back-compat floor: without a schema the assertion behaves exactly as it
    /// always did — presence of a JSON value, nothing more.
    #[test]
    fn no_schema_is_unchanged() {
        assert!(eval_with_schema(r#"{"anything": true}"#, None).passed);
        assert!(!eval_with_schema("no json here", None).passed);
    }

    /// A missing JSON value must fail on its own terms rather than being
    /// reported as a schema mismatch, which would send the reader to the wrong
    /// half of the assertion.
    #[test]
    fn absent_json_fails_before_the_schema_is_consulted() {
        let schema = json!({"type": "object"});
        let outcome = eval_with_schema("prose only", Some(schema));
        assert!(!outcome.passed);
        assert!(
            outcome.reason.contains("no JSON value"),
            "{}",
            outcome.reason
        );
    }

    /// `negate` composes for free, so `not-contains-json` with a schema means
    /// "no JSON here matches this shape".
    #[test]
    fn negate_inverts_a_schema_match() {
        let engine = TemplateEngine::new();
        let vars = json!({});
        let metrics = MetricCtx::default();
        let schemas = SchemaCache::new();
        let outcome = evaluate_local(
            &Assert {
                weight: 1.0,
                negate: true,
                kind: AssertKind::ContainsJson {
                    schema: Some(Val::Raw(json!({"type": "object"}))),
                },
            },
            &Output::Text(r#"{"a": 1}"#.into()),
            &EvalCtx {
                engine: &engine,
                vars: &vars,
                metrics: &metrics,
                schemas: &schemas,
                tool_calls: &[],
                empty_reason: None,
            },
        )
        .unwrap();
        assert!(!outcome.passed, "a match under negate must fail");
    }

    /// The guard that fails *open*. Nothing compiles an assertion's `schema:`
    /// before the run, so a malformed one reaches the evaluator — and because a
    /// compile failure used to be an ordinary failure, `negate` flipped it into
    /// a full-score pass. `not-contains-json` with a remote `$ref` (which
    /// `default-features = false` makes a hard compile error) therefore reported
    /// a green check, with reason `negated: invalid JSON Schema: …`, for an
    /// assertion that never ran.
    #[test]
    fn negate_cannot_turn_an_uncompilable_schema_into_a_pass() {
        let engine = TemplateEngine::new();
        let vars = json!({});
        let metrics = MetricCtx::default();
        let schemas = SchemaCache::new();
        let evaluate = |negate: bool| {
            evaluate_local(
                &Assert {
                    weight: 1.0,
                    negate,
                    kind: AssertKind::ContainsJson {
                        schema: Some(Val::Raw(json!({"$ref": "https://example.invalid/s.json"}))),
                    },
                },
                &Output::Text(r#"{"a": 1}"#.into()),
                &EvalCtx {
                    engine: &engine,
                    vars: &vars,
                    metrics: &metrics,
                    schemas: &schemas,
                    tool_calls: &[],
                    empty_reason: None,
                },
            )
            .unwrap()
        };

        for negate in [false, true] {
            let outcome = evaluate(negate);
            assert!(
                !outcome.passed && outcome.score == 0.0,
                "negate={negate} scored {} / passed={}",
                outcome.score,
                outcome.passed
            );
            assert!(
                outcome.unevaluable,
                "a schema that will not compile is a broken assertion, not an \
                 unsatisfied one — the runner reports it as an error"
            );
            assert!(
                !outcome.reason.starts_with("negated:"),
                "{}",
                outcome.reason
            );
        }
    }
}

mod tool_call_tests {
    use super::*;
    use crate::config::Assert;
    use crate::result::ToolCall;
    use crate::val::Val;

    fn call(name: &str, args: serde_json::Value) -> ToolCall {
        ToolCall {
            id: None,
            name: name.to_string(),
            arguments: args,
        }
    }

    fn eval(kind: AssertKind, negate: bool, calls: &[ToolCall]) -> AssertOutcome {
        let engine = TemplateEngine::new();
        let vars = serde_json::json!({"city": "Reykjavik"});
        let metrics = MetricCtx::default();
        let schemas = crate::jsonschema_cache::SchemaCache::default();
        let assert = Assert {
            weight: 1.0,
            negate,
            kind,
        };
        evaluate_local(
            &assert,
            &Output::Text(String::new()),
            &EvalCtx {
                engine: &engine,
                vars: &vars,
                metrics: &metrics,
                schemas: &schemas,
                tool_calls: calls,
                empty_reason: None,
            },
        )
        .expect("tool-call is a local assertion")
    }

    fn named(name: &str) -> AssertKind {
        AssertKind::ToolCall {
            name: name.to_string(),
            args: None,
            schema: None,
        }
    }

    /// The gap this closes: a case whose right answer is a tool call had no
    /// prose to grade, so every text assertion scored zero and it read as a
    /// model failure rather than an evaluation that could not see the answer.
    #[test]
    fn a_reported_call_satisfies_an_assertion_about_it() {
        let calls = vec![call(
            "get_weather",
            serde_json::json!({"city": "Reykjavik"}),
        )];
        assert!(eval(named("get_weather"), false, &calls).passed);
        assert!(!eval(named("get_forecast"), false, &calls).passed);
    }

    /// The failure message names what *was* called. "tool `x` was not called"
    /// alone leaves you opening the case to find out what happened instead.
    #[test]
    fn the_failure_names_the_tools_that_were_called() {
        let calls = vec![call("delete_user", serde_json::json!({}))];
        let outcome = eval(named("archive_user"), false, &calls);
        assert!(outcome.reason.contains("delete_user"), "{}", outcome.reason);

        let outcome = eval(named("archive_user"), false, &[]);
        assert!(
            outcome.reason.contains("no tools were called"),
            "{}",
            outcome.reason
        );
    }

    /// The negative is the point of the feature as much as the positive: a
    /// safety eval asserts the model did *not* reach for the destructive tool.
    #[test]
    fn negation_asserts_a_tool_was_not_called() {
        let calls = vec![call("read_user", serde_json::json!({}))];
        assert!(eval(named("delete_user"), true, &calls).passed);
        assert!(!eval(named("read_user"), true, &calls).passed);
    }

    /// A subset match, not equality: an assertion should not have to restate
    /// every argument to pin the one that matters, and a tool gaining an
    /// optional argument must not break every assertion about it.
    #[test]
    fn args_match_a_subset_and_are_rendered() {
        let calls = vec![call(
            "get_weather",
            serde_json::json!({"city": "Reykjavik", "units": "metric"}),
        )];
        let kind = |args: serde_json::Value| AssertKind::ToolCall {
            name: "get_weather".into(),
            args: Some(Val::Tpl(args)),
            schema: None,
        };
        // Templated, because an expected argument is a per-case value.
        assert!(
            eval(
                kind(serde_json::json!({"city": "{{ city }}"})),
                false,
                &calls
            )
            .passed
        );
        assert!(eval(kind(serde_json::json!({"units": "metric"})), false, &calls).passed);
        assert!(
            !eval(
                kind(serde_json::json!({"units": "imperial"})),
                false,
                &calls
            )
            .passed
        );

        let outcome = eval(kind(serde_json::json!({"nope": 1})), false, &calls);
        assert!(outcome.reason.contains("`nope`"), "{}", outcome.reason);
    }

    /// A model may call the same tool twice; requiring the *first* to match
    /// would make an assertion depend on an ordering the prompt never asked for.
    #[test]
    fn any_matching_call_satisfies_the_assertion() {
        let calls = vec![
            call("search", serde_json::json!({"q": "wrong"})),
            call("search", serde_json::json!({"q": "right"})),
        ];
        let kind = AssertKind::ToolCall {
            name: "search".into(),
            args: Some(Val::Raw(serde_json::json!({"q": "right"}))),
            schema: None,
        };
        assert!(eval(kind, false, &calls).passed);
    }

    #[test]
    fn a_schema_constrains_the_arguments() {
        let schema = serde_json::json!({
            "type": "object",
            "required": ["city"],
            "properties": {"city": {"type": "string"}},
        });
        let kind = AssertKind::ToolCall {
            name: "get_weather".into(),
            args: None,
            schema: Some(Val::Raw(schema)),
        };
        assert!(
            eval(
                kind.clone(),
                false,
                &[call("get_weather", serde_json::json!({"city": "Oslo"}))]
            )
            .passed
        );
        assert!(
            !eval(
                kind,
                false,
                &[call("get_weather", serde_json::json!({"city": 7}))]
            )
            .passed
        );
    }

    /// The same fail-open hole as `not-contains-json`, on the assertion added in
    /// the same branch: a `schema:` that will not compile made `not-tool-call`
    /// score a full 1.0 for a call it never validated.
    #[test]
    fn negate_cannot_turn_an_uncompilable_tool_schema_into_a_pass() {
        let kind = || AssertKind::ToolCall {
            name: "get_weather".into(),
            args: None,
            schema: Some(Val::Raw(
                serde_json::json!({"$ref": "https://example.invalid/s.json"}),
            )),
        };
        let calls = [call("get_weather", serde_json::json!({"city": "Oslo"}))];
        for negate in [false, true] {
            let outcome = eval(kind(), negate, &calls);
            assert!(
                !outcome.passed && outcome.score == 0.0,
                "negate={negate}: {outcome:?}"
            );
            assert!(outcome.unevaluable, "{outcome:?}");
        }
    }
}

/// The vacuous-pass guard: a negated assertion must not earn a pass merely
/// because the provider produced nothing to judge.
mod vacuous_pass_tests {
    use super::*;
    use crate::empty::EmptyReason;
    use crate::result::ToolCall;
    use crate::val::Val;

    /// One assert over an output the provider produced nothing gradeable in.
    /// The metrics are populated so every metric bound below *fails*, and
    /// therefore passes under negation — exactly the pass the guard must leave
    /// alone.
    fn eval_empty(
        kind: AssertKind,
        negate: bool,
        calls: &[ToolCall],
        out: Output,
    ) -> AssertOutcome {
        let engine = TemplateEngine::new();
        let vars = json!({});
        let metrics = MetricCtx {
            latency_ms: 12,
            cost_usd: Some(1.0),
            total_tokens: Some(100),
            billable_tokens: Some(100),
        };
        let schemas = crate::jsonschema_cache::SchemaCache::new();
        let refusal = EmptyReason::new(EmptyReason::REFUSAL);
        evaluate_local(
            &Assert {
                weight: 1.0,
                negate,
                kind,
            },
            &out,
            &EvalCtx {
                engine: &engine,
                vars: &vars,
                metrics: &metrics,
                schemas: &schemas,
                tool_calls: calls,
                empty_reason: Some(&refusal),
            },
        )
        .expect("a local assertion yields an outcome")
    }

    /// "The forbidden content is absent" is not evidence of compliance when
    /// nothing was produced at all.
    #[test]
    fn a_negated_assert_cannot_pass_vacuously_when_the_output_is_empty() {
        let outcome = eval_empty(
            AssertKind::Contains {
                value: "forbidden".into(),
            },
            true,
            &[],
            Output::Text(String::new()),
        );
        assert!(!outcome.passed, "{outcome:?}");
        assert_eq!(outcome.score, 0.0, "{outcome:?}");
        assert!(!outcome.unevaluable, "{outcome:?}");
        assert!(
            outcome.reason.contains("vacuously") && outcome.reason.contains("refusal"),
            "{}",
            outcome.reason
        );
    }

    /// The motivating hole: a refusal calls no tools, so `not-tool-call`
    /// scored a full 1.0 for a case the model never attempted.
    #[test]
    fn not_tool_call_fails_on_a_refusal_instead_of_passing_vacuously() {
        let outcome = eval_empty(
            AssertKind::ToolCall {
                name: "get_weather".into(),
                args: None,
                schema: None,
            },
            true,
            &[],
            Output::Text(String::new()),
        );
        assert!(!outcome.passed && outcome.score == 0.0, "{outcome:?}");
        assert!(!outcome.unevaluable, "{outcome:?}");
    }

    /// `tool_use_only` is an empty *text* output, and a `tool-call` assert
    /// never read that text: the calls it judges were reported. Denying this
    /// pass would fail every tool-answering case that carries a `not-tool-call`
    /// guard rail.
    #[test]
    fn not_tool_call_still_passes_when_the_model_did_call_a_tool() {
        let calls = [ToolCall {
            id: None,
            name: "get_weather".into(),
            arguments: json!({"city": "Oslo"}),
        }];
        let outcome = eval_empty(
            AssertKind::ToolCall {
                name: "delete_everything".into(),
                args: None,
                schema: None,
            },
            true,
            &calls,
            Output::Text(String::new()),
        );
        assert!(outcome.passed && outcome.score == 1.0, "{outcome:?}");
    }

    /// A negated latency bound is still a true statement about latency when
    /// the output is empty — the guard is scoped to content asserts.
    #[test]
    fn a_negated_metric_assert_is_exempt_from_the_vacuous_pass_guard() {
        for kind in [
            AssertKind::Latency { max: 1 },
            AssertKind::Tokens {
                max: 1,
                count: None,
            },
            AssertKind::Cost { max: 0.0 },
        ] {
            let outcome = eval_empty(kind, true, &[], Output::Text(String::new()));
            assert!(
                outcome.passed && outcome.score == 1.0,
                "a metric assert keeps its negated pass: {outcome:?}"
            );
        }
    }

    /// The guard only refuses passes; a negated assert that already failed
    /// keeps its own diagnosis.
    #[test]
    fn the_guard_leaves_a_failing_negated_assert_untouched() {
        // `length` over "" satisfies `max: 10`, so negation fails it.
        let outcome = eval_empty(
            AssertKind::Length {
                min: None,
                max: Some(10),
            },
            true,
            &[],
            Output::Text(String::new()),
        );
        assert!(!outcome.passed, "{outcome:?}");
        assert!(
            outcome.reason.starts_with("negated:"),
            "the original reason survives: {}",
            outcome.reason
        );
    }

    /// `unevaluable` is about a broken assertion, not about the output, and
    /// must keep reaching the runner as an error rather than a failure.
    #[test]
    fn an_unevaluable_negated_assert_still_errors_not_fails() {
        let outcome = eval_empty(
            AssertKind::ContainsJson {
                schema: Some(Val::Raw(json!({"$ref": "https://example.invalid/s.json"}))),
            },
            true,
            &[],
            // Structurally empty (`classify_blank` calls it blank) but still
            // JSON, so the assertion reaches its uncompilable schema.
            Output::Json(json!({})),
        );
        assert!(outcome.unevaluable, "{outcome:?}");
        assert!(!outcome.passed && outcome.score == 0.0, "{outcome:?}");
        assert!(!outcome.reason.contains("vacuously"), "{}", outcome.reason);
    }
}
