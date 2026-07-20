//! Local (deterministic) assertion evaluation.
//!
//! These asserts need no network and run first, so a failing one can
//! short-circuit the expensive LLM grader. Asserts that require an external call
//! (`exec`, `llm-rubric`, `similar`) are handled by the runner's async path and
//! return `None` here.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use ts_rs::TS;

use crate::assertion::AssertOutcome;
use crate::config::{Assert, AssertKind};
use crate::template::TemplateEngine;
use crate::types::Output;

/// Metrics available to budget assertions after a provider call.
#[derive(Debug, Clone, Default)]
pub struct MetricCtx {
    pub latency_ms: u64,
    pub cost_usd: Option<f64>,
    pub total_tokens: Option<u64>,
}

/// A field-less mirror of every [`AssertKind`] variant, recorded in results as
/// the stable "kind" name (matches the config `type` tag). Kept in sync with
/// `AssertKind` by [`AssertKind::name`] and the pin test below.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum AssertName {
    Contains,
    Icontains,
    IcontainsAny,
    Regex,
    Equals,
    StartsWith,
    IsJson,
    ContainsJson,
    Length,
    Jinja,
    Exec,
    LlmRubric,
    Cost,
    Latency,
    Tokens,
    Similar,
}

impl AssertName {
    /// The wire string for this kind (identical to its serde encoding).
    pub fn as_str(self) -> &'static str {
        match self {
            AssertName::Contains => "contains",
            AssertName::Icontains => "icontains",
            AssertName::IcontainsAny => "icontains-any",
            AssertName::Regex => "regex",
            AssertName::Equals => "equals",
            AssertName::StartsWith => "starts-with",
            AssertName::IsJson => "is-json",
            AssertName::ContainsJson => "contains-json",
            AssertName::Length => "length",
            AssertName::Jinja => "jinja",
            AssertName::Exec => "exec",
            AssertName::LlmRubric => "llm-rubric",
            AssertName::Cost => "cost",
            AssertName::Latency => "latency",
            AssertName::Tokens => "tokens",
            AssertName::Similar => "similar",
        }
    }
}

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

/// Evaluate a local assertion. Returns `None` for asserts that need the async
/// path (their `is_local` is false).
pub fn evaluate_local(
    assert: &Assert,
    output: &Output,
    engine: &TemplateEngine,
    vars: &Json,
    metrics: &MetricCtx,
) -> Option<AssertOutcome> {
    if !is_local(&assert.kind) {
        return None;
    }
    let outcome = evaluate_kind(&assert.kind, output, engine, vars, metrics);
    Some(outcome.negated(assert.negate))
}

fn evaluate_kind(
    kind: &AssertKind,
    output: &Output,
    engine: &TemplateEngine,
    vars: &Json,
    metrics: &MetricCtx,
) -> AssertOutcome {
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
            // Schema validation is added with the jsonschema dependency later;
            // for now this checks for the presence of a JSON value.
            let _ = schema;
            cond(
                extract_json(&text).is_some(),
                "output contains JSON",
                "output contains no JSON value",
            )
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
        AssertKind::Tokens { max } => match metrics.total_tokens {
            Some(tokens) => cond(
                tokens <= *max,
                format!("tokens {tokens} <= {max}"),
                format!("tokens {tokens} > {max}"),
            ),
            None => AssertOutcome::pass("tokens not reported; budget not enforced"),
        },
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
        evaluate_local(
            assert,
            &Output::Text(out.into()),
            &TemplateEngine::new(),
            &json!({}),
            &MetricCtx::default(),
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
        };
        let out = Output::Text("x".into());
        let eng = TemplateEngine::new();
        let vars = json!({});
        let lat = evaluate_local(
            &a(AssertKind::Latency { max: 1000 }),
            &out,
            &eng,
            &vars,
            &metrics,
        )
        .unwrap();
        assert!(lat.passed);
        let toks = evaluate_local(
            &a(AssertKind::Tokens { max: 1000 }),
            &out,
            &eng,
            &vars,
            &metrics,
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
        assert!(evaluate_local(
            &assert,
            &Output::Text("x".into()),
            &TemplateEngine::new(),
            &json!({}),
            &MetricCtx::default()
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
            },
            AssertKind::LlmRubric {
                value: String::new(),
                grader: None,
                threshold: None,
                params: None,
            },
            AssertKind::Cost { max: 0.0 },
            AssertKind::Latency { max: 0 },
            AssertKind::Tokens { max: 0 },
            AssertKind::Similar {
                value: Val::Raw(Json::Null),
                threshold: None,
            },
        ];
        assert_eq!(variants.len(), 16, "update this test when adding a variant");

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
