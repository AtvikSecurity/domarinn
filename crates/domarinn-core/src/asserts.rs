//! Local (deterministic) assertion evaluation.
//!
//! These asserts need no network and run first, so a failing one can
//! short-circuit the expensive LLM grader. Asserts that require an external call
//! (`exec`, `llm-rubric`, `similar`) are handled by the runner's async path and
//! return `None` here.

use serde_json::Value as Json;

use crate::assertion::AssertOutcome;
use crate::config::{Assert, AssertKind};
use crate::template::TemplateEngine;
use crate::types::Output;

/// Metrics available to budget assertions after a provider call.
#[derive(Debug, Clone, Default)]
pub struct MetricCtx {
    pub latency_ms: u64,
    pub cost_usd: Option<f64>,
    /// Input plus output — what `tokens` grades by default.
    pub total_tokens: Option<u64>,
    /// Everything the provider bills for, cache traffic included. Selected by
    /// `tokens: {count: billable}`.
    pub billable_tokens: Option<u64>,
}

/// Everything a local assertion needs beyond the assertion and the output.
///
/// A struct rather than a parameter list: the shared state an assertion may
/// need is open-ended (a compiled-schema cache is next), and threading it one
/// argument at a time is how the runner's own assert entry point ended up with
/// eight parameters. Borrowed, never owned — this is built per cell and thrown
/// away.
pub struct EvalCtx<'a> {
    pub engine: &'a TemplateEngine,
    pub vars: &'a Json,
    pub metrics: &'a MetricCtx,
    /// Compiled JSON Schemas for `contains-json`, memoized for the run. See
    /// [`crate::jsonschema_cache`] for why it is threaded rather than global.
    pub schemas: &'a crate::jsonschema_cache::SchemaCache,
    /// The tool calls this cell's provider reported, in order.
    ///
    /// Beside `output` rather than inside it: `Output` is what gets *scored* as
    /// text, and folding calls into it would change what every existing
    /// assertion sees — and what a cache key hashes.
    pub tool_calls: &'a [crate::result::ToolCall],
}

// `AssertName` lives in `domarinn-types`: it is a wire value (it appears on
// every stored `AssertResult`), whereas the evaluation logic in this module is
// engine-only. Re-exported so `crate::asserts::AssertName` keeps resolving.
pub use domarinn_types::assert_name::AssertName;

impl AssertKind {
    /// The stable kind name recorded in results (matches the config `type`).
    pub fn name(&self) -> AssertName {
        match self {
            AssertKind::Contains { .. } => AssertName::Contains,
            AssertKind::Icontains { .. } => AssertName::Icontains,
            AssertKind::IcontainsAny { .. } => AssertName::IcontainsAny,
            AssertKind::Regex { .. } => AssertName::Regex,
            AssertKind::Equals { .. } => AssertName::Equals,
            AssertKind::StartsWith { .. } => AssertName::StartsWith,
            AssertKind::IsJson => AssertName::IsJson,
            AssertKind::ContainsJson { .. } => AssertName::ContainsJson,
            AssertKind::Length { .. } => AssertName::Length,
            AssertKind::Jinja { .. } => AssertName::Jinja,
            AssertKind::Exec { .. } => AssertName::Exec,
            AssertKind::LlmRubric { .. } => AssertName::LlmRubric,
            AssertKind::Cost { .. } => AssertName::Cost,
            AssertKind::Latency { .. } => AssertName::Latency,
            AssertKind::Tokens { .. } => AssertName::Tokens,
            AssertKind::Similar { .. } => AssertName::Similar,
            AssertKind::ToolCall { .. } => AssertName::ToolCall,
        }
    }
}

/// Whether an assertion is evaluated locally (deterministically) here, versus
/// needing the runner's async path.
pub fn is_local(kind: &AssertKind) -> bool {
    !matches!(
        kind,
        AssertKind::Exec { .. } | AssertKind::LlmRubric { .. } | AssertKind::Similar { .. }
    )
}

/// Whether one reported call satisfies an assertion's `args`/`schema`
/// constraints, or why it does not.
///
/// `args` is a **subset** match: every key given must be present and deep-equal.
/// An assertion should not have to restate an entire argument object to pin the
/// one value that matters, and requiring equality would make a tool gaining an
/// optional argument break every assertion about it.
fn tool_call_matches(
    call: &crate::result::ToolCall,
    args: Option<&crate::val::Val>,
    schema: Option<&crate::val::Val>,
    ctx: &EvalCtx<'_>,
) -> Result<(), String> {
    if let Some(args) = args {
        // Rendered, unlike `schema`: an expected argument is a per-case value
        // (`{"city": "{{ city }}"}`), which is exactly what a template is for.
        let expected = ctx
            .engine
            .render_val(args, ctx.vars)
            .map_err(|e| format!("rendering expected args: {e}"))?;
        let Some(expected) = expected.as_object() else {
            return Err("`args` must be an object".to_string());
        };
        for (k, want) in expected {
            match call.arguments.get(k) {
                None => return Err(format!("argument `{k}` was not passed")),
                Some(got) if got != want => {
                    return Err(format!("argument `{k}` was {got}, expected {want}"))
                }
                Some(_) => {}
            }
        }
    }
    if let Some(schema) = schema {
        let outcome = crate::jsonschema_cache::validate_against(
            &call.arguments,
            schema.as_json(),
            ctx.schemas,
        );
        if !outcome.passed {
            return Err(outcome.reason);
        }
    }
    Ok(())
}

/// Evaluate a local assertion. Returns `None` for asserts that need the async
/// path (their `is_local` is false).
pub fn evaluate_local(
    assert: &Assert,
    output: &Output,
    ctx: &EvalCtx<'_>,
) -> Option<AssertOutcome> {
    if !is_local(&assert.kind) {
        return None;
    }
    let outcome = evaluate_kind(&assert.kind, output, ctx);
    Some(outcome.negated(assert.negate))
}

fn evaluate_kind(kind: &AssertKind, output: &Output, ctx: &EvalCtx<'_>) -> AssertOutcome {
    let EvalCtx {
        engine,
        vars,
        metrics,
        schemas,
        ..
    } = ctx;
    let text = output.as_text();
    match kind {
        AssertKind::Contains { value } => cond(
            text.contains(value.as_str()),
            format!("output contains \"{value}\""),
            format!("output does not contain \"{value}\""),
        ),
        AssertKind::Icontains { value } => cond(
            text.to_lowercase().contains(&value.to_lowercase()),
            format!("output contains \"{value}\" (case-insensitive)"),
            format!("output does not contain \"{value}\" (case-insensitive)"),
        ),
        AssertKind::IcontainsAny { values } => {
            let lower = text.to_lowercase();
            let hit = values.iter().find(|v| lower.contains(&v.to_lowercase()));
            match hit {
                Some(v) => AssertOutcome::pass(format!("output contains \"{v}\"")),
                None => AssertOutcome::fail(format!("output contains none of {values:?}")),
            }
        }
        AssertKind::Regex { value } => match regex::Regex::new(value) {
            Ok(re) => cond(
                re.is_match(&text),
                format!("output matches /{value}/"),
                format!("output does not match /{value}/"),
            ),
            Err(e) => AssertOutcome::fail(format!("invalid regex /{value}/: {e}")),
        },
        AssertKind::Equals { value } => {
            let rendered = engine
                .render_val(value, vars)
                .unwrap_or_else(|_| value.as_json().clone());
            let matches = match &rendered {
                Json::String(s) => text == s.as_str(),
                other => output.as_json().as_ref() == Some(other),
            };
            cond(
                matches,
                "output equals expected",
                "output does not equal expected",
            )
        }
        AssertKind::StartsWith { value } => cond(
            text.starts_with(value.as_str()),
            format!("output starts with \"{value}\""),
            format!("output does not start with \"{value}\""),
        ),
        AssertKind::IsJson => cond(
            output.as_json().is_some(),
            "output is valid JSON",
            "output is not valid JSON",
        ),
        AssertKind::ContainsJson { schema } => {
            let Some(found) = extract_json(&text) else {
                return AssertOutcome::fail("output contains no JSON value");
            };
            match schema {
                None => AssertOutcome::pass("output contains JSON"),
                // Deliberately *not* rendered as a template. A schema is a
                // contract, not a per-case value: rendering it would make the
                // memo key case-dependent (defeating the cache) and open a
                // template surface over a whole document for no gain.
                Some(schema) => {
                    crate::jsonschema_cache::validate_against(&found, schema.as_json(), schemas)
                }
            }
        }
        AssertKind::ToolCall { name, args, schema } => {
            let matching: Vec<&crate::result::ToolCall> =
                ctx.tool_calls.iter().filter(|c| &c.name == name).collect();
            if matching.is_empty() {
                let called = if ctx.tool_calls.is_empty() {
                    "no tools were called".to_string()
                } else {
                    format!(
                        "called: {}",
                        ctx.tool_calls
                            .iter()
                            .map(|c| c.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                };
                return AssertOutcome::fail(format!("tool `{name}` was not called ({called})"));
            }
            // Any matching call satisfying every constraint passes. A model may
            // legitimately call the same tool more than once, and requiring the
            // *first* to match would make an assertion depend on an ordering
            // the prompt never asked for.
            let mut last_reason = String::new();
            for call in &matching {
                match tool_call_matches(call, args.as_ref(), schema.as_ref(), ctx) {
                    Ok(()) => {
                        return AssertOutcome::pass(format!("tool `{name}` was called as expected"))
                    }
                    Err(why) => last_reason = why,
                }
            }
            AssertOutcome::fail(format!(
                "tool `{name}` was called {} time(s), none matching: {last_reason}",
                matching.len()
            ))
        }
        AssertKind::Length { min, max } => {
            let len = text.chars().count() as u64;
            let below = min.map(|m| len < m).unwrap_or(false);
            let above = max.map(|m| len > m).unwrap_or(false);
            if below {
                AssertOutcome::fail(format!("length {len} < min {}", min.unwrap()))
            } else if above {
                AssertOutcome::fail(format!("length {len} > max {}", max.unwrap()))
            } else {
                AssertOutcome::pass(format!("length {len} within bounds"))
            }
        }
        AssertKind::Jinja { value } => {
            let ctx = assert_context(output, vars);
            match engine.eval_bool(value, &ctx) {
                Ok(true) => AssertOutcome::pass(format!("expression `{value}` is true")),
                Ok(false) => AssertOutcome::fail(format!("expression `{value}` is false")),
                Err(e) => AssertOutcome::fail(format!("expression `{value}` errored: {e}")),
            }
        }
        AssertKind::Cost { max } => match metrics.cost_usd {
            Some(cost) => cond(
                cost <= *max,
                format!("cost ${cost:.6} <= ${max:.6}"),
                format!("cost ${cost:.6} > ${max:.6}"),
            ),
            None => AssertOutcome::pass("cost not reported; budget not enforced"),
        },
        AssertKind::Latency { max } => cond(
            metrics.latency_ms <= *max,
            format!("latency {}ms <= {max}ms", metrics.latency_ms),
            format!("latency {}ms > {max}ms", metrics.latency_ms),
        ),
        AssertKind::Tokens { max, count } => {
            // `total` is the default so an existing `tokens: {max: N}` keeps
            // meaning exactly what it always meant. Opting in to `billable`
            // is how a suite budgets cache traffic too.
            let billable = matches!(count, Some(crate::config::TokenCount::Billable));
            let measured = if billable {
                metrics.billable_tokens
            } else {
                metrics.total_tokens
            };
            let label = if billable {
                "billable tokens"
            } else {
                "tokens"
            };
            match measured {
                Some(tokens) => cond(
                    tokens <= *max,
                    format!("{label} {tokens} <= {max}"),
                    format!("{label} {tokens} > {max}"),
                ),
                None => AssertOutcome::pass("tokens not reported; budget not enforced"),
            }
        }
        // Non-local asserts never reach here (guarded by is_local).
        AssertKind::Exec { .. } | AssertKind::LlmRubric { .. } | AssertKind::Similar { .. } => {
            AssertOutcome::fail("internal: non-local assert routed to local path")
        }
    }
}

fn cond(
    pass: bool,
    pass_reason: impl Into<String>,
    fail_reason: impl Into<String>,
) -> AssertOutcome {
    if pass {
        AssertOutcome::pass(pass_reason)
    } else {
        AssertOutcome::fail(fail_reason)
    }
}

/// Build the context for a `jinja` assertion: vars plus `output` / `output_json`.
fn assert_context(output: &Output, vars: &Json) -> Json {
    let mut obj = vars.as_object().cloned().unwrap_or_default();
    obj.insert("output".into(), Json::String(output.as_text().into_owned()));
    if let Some(j) = output.as_json() {
        obj.insert("output_json".into(), j);
    }
    obj.insert("vars".into(), vars.clone());
    Json::Object(obj)
}

/// Find the first balanced JSON object or array embedded in text.
fn extract_json(text: &str) -> Option<Json> {
    if let Ok(v) = serde_json::from_str::<Json>(text.trim()) {
        return Some(v);
    }
    let start = text.find(['{', '['])?;
    let open = text.as_bytes()[start];
    let close = if open == b'{' { b'}' } else { b']' };
    let mut depth = 0i32;
    for (i, b) in text.bytes().enumerate().skip(start) {
        if b == open {
            depth += 1;
        } else if b == close {
            depth -= 1;
            if depth == 0 {
                return serde_json::from_str(&text[start..=i]).ok();
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
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
}

#[cfg(test)]
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
            },
        )
        .unwrap();
        assert!(!outcome.passed, "a match under negate must fail");
    }
}

#[cfg(test)]
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
}
