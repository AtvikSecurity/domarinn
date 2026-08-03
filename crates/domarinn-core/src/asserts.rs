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
    /// Why this cell's output has nothing gradeable in it, if it does not.
    /// Read only by the vacuous-pass guard — see
    /// [`AssertOutcome::deny_vacuous_negated_pass`].
    pub empty_reason: Option<&'a crate::empty::EmptyReason>,
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

/// Whether the vacuous-pass guard applies to this assertion — see
/// [`AssertOutcome::deny_vacuous_negated_pass`]. Shared by both seams (local
/// here, graded in `runner_asserts`) so they cannot drift, and takes the
/// evidence rather than a context type because each seam carries its own.
///
/// Three exemptions, all cases where the guard's premise — nothing was
/// produced — does not actually hold:
///
/// - **Metric asserts** never read the output. A negated latency bound is
///   still a true statement about latency when nothing came back.
/// - **Any assert on a response that reported tool calls.** `tool_use_only` —
///   the model called a tool and said nothing else — is an empty *text*
///   output, and the model did act: `not-tool-call: delete_everything` judges
///   the calls that *were* reported, and a rubric judging behaviour is shown
///   them too. The hole the guard exists to close is a refusal, which reports
///   no calls at all.
/// - **An output that is not actually blank.** `empty_reason` is a claim, and
///   an exec child's claim is honoured verbatim even beside real text — so the
///   guard re-checks the output it is about to call empty. A negated assert
///   over genuine content evaluated genuine content; failing it with "output
///   was empty" would be both wrong and self-contradictory next to the
///   positive asserts on the same case, which judged that text and passed.
pub fn guard_applies(
    kind: &AssertKind,
    output: &Output,
    tool_calls: &[crate::result::ToolCall],
) -> bool {
    tool_calls.is_empty()
        && crate::empty::classify_blank(output).is_some()
        && !matches!(
            kind,
            AssertKind::Cost { .. } | AssertKind::Latency { .. } | AssertKind::Tokens { .. }
        )
}

/// Apply `negate`, then the vacuous-pass guard — the one composition both
/// seams (local below, graded in `runner_asserts`) go through. The guard's
/// *predicate* was already shared via [`guard_applies`]; sharing the
/// composition too is what stops a future third seam calling
/// [`AssertOutcome::negated`] without the follow-up and silently reopening the
/// vacuous-pass hole for that path alone. Ordering matters at the graded seam:
/// this must run before the outcome is scored, so the score a case is graded
/// on and the result its drawer shows agree.
pub fn negate_and_guard(
    outcome: AssertOutcome,
    assert: &Assert,
    output: &Output,
    tool_calls: &[crate::result::ToolCall],
    empty_reason: Option<&crate::empty::EmptyReason>,
) -> AssertOutcome {
    let outcome = outcome.negated(assert.negate);
    if guard_applies(&assert.kind, output, tool_calls) {
        outcome.deny_vacuous_negated_pass(assert.negate, empty_reason)
    } else {
        outcome
    }
}

/// Whether one reported call satisfies an assertion's `args`/`schema`
/// constraints, or why it does not.
///
/// `args` is a **subset** match: every key given must be present and deep-equal.
/// An assertion should not have to restate an entire argument object to pin the
/// one value that matters, and requiring equality would make a tool gaining an
/// optional argument break every assertion about it.
/// Why one reported call did not satisfy an assertion, and whether the
/// assertion is *broken* rather than merely unsatisfied.
///
/// The flag has to survive out of here: a `schema:` that will not compile is a
/// config error for every call the model made, and collapsing it into an
/// ordinary mismatch lets `negate` turn `not-tool-call` into a full-score pass
/// over an assertion that never ran.
struct CallMismatch {
    reason: String,
    unevaluable: bool,
}

impl From<String> for CallMismatch {
    fn from(reason: String) -> Self {
        CallMismatch {
            reason,
            unevaluable: false,
        }
    }
}

fn tool_call_matches(
    call: &crate::result::ToolCall,
    args: Option<&crate::val::Val>,
    schema: Option<&crate::val::Val>,
    ctx: &EvalCtx<'_>,
) -> Result<(), CallMismatch> {
    if let Some(args) = args {
        // Rendered, unlike `schema`: an expected argument is a per-case value
        // (`{"city": "{{ city }}"}`), which is exactly what a template is for.
        let expected = ctx
            .engine
            .render_val(args, ctx.vars)
            // A template that will not render is a broken assertion, not a call
            // that failed to match.
            .map_err(|e| CallMismatch {
                reason: format!("rendering expected args: {e}"),
                unevaluable: true,
            })?;
        let Some(expected) = expected.as_object() else {
            return Err(CallMismatch {
                reason: "`args` must be an object".to_string(),
                unevaluable: true,
            });
        };
        for (k, want) in expected {
            match call.arguments.get(k) {
                None => return Err(format!("argument `{k}` was not passed").into()),
                Some(got) if got != want => {
                    return Err(format!("argument `{k}` was {got}, expected {want}").into())
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
            return Err(CallMismatch {
                reason: outcome.reason,
                unevaluable: outcome.unevaluable,
            });
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
    Some(negate_and_guard(
        evaluate_kind(&assert.kind, output, ctx),
        assert,
        output,
        ctx.tool_calls,
        ctx.empty_reason,
    ))
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
            Err(e) => AssertOutcome::unevaluable(format!("invalid regex /{value}/: {e}")),
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
                    // A broken assertion is broken for every call the model
                    // made, so there is nothing to keep trying — and reporting
                    // it as an ordinary mismatch would let `negate` flip a
                    // schema that will not compile into a full-score pass.
                    Err(m) if m.unevaluable => {
                        return AssertOutcome::unevaluable(format!(
                            "tool `{name}` assertion cannot be evaluated: {}",
                            m.reason
                        ))
                    }
                    Err(m) => last_reason = m.reason,
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
                Err(e) => AssertOutcome::unevaluable(format!("expression `{value}` errored: {e}")),
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
            AssertOutcome::unevaluable("internal: non-local assert routed to local path")
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
#[path = "asserts_tests.rs"]
mod tests;
