//! Assertion evaluation for a single case: run the local (deterministic)
//! asserts, decide whether the deferred (graded) ones can still change the
//! outcome, and shape each verdict into an [`AssertResult`].
//!
//! Split out of `runner.rs` — which orchestrates the matrix and owns the
//! provider/cache path — so each file stays under the per-file line ratchet
//! (`tests/file_length.rs`). Included as a private child module of `runner`,
//! hence `super::AssertGrader` and the `pub(super)` seams.

use std::path::Path;

use serde_json::Value as Json;

use crate::assertion::AssertOutcome;
use crate::asserts::{evaluate_local, is_local, MetricCtx};
use crate::config::{Assert, AssertKind};
use crate::error_class::ErrorClass;
use crate::errors::Classify;
use crate::result::{AssertResult, AssertStatus};
use crate::scoring::{remaining_can_change_outcome, Scored};
use crate::template::TemplateEngine;
use crate::types::Output;

use super::AssertGrader;

/// Evaluate all asserts: local first, then (if they can still change the
/// outcome) the graded ones; otherwise mark them skipped.
#[allow(clippy::too_many_arguments)]
pub(super) async fn evaluate_asserts(
    asserts: &[Assert],
    output: &Output,
    vars: &Json,
    engine: &TemplateEngine,
    grader: Option<&dyn AssertGrader>,
    base_dir: &Path,
    metrics: &MetricCtx,
    threshold: Option<f64>,
) -> (Vec<AssertResult>, Vec<Scored>, Vec<ErrorClass>) {
    // Slot results by original index so output order matches config order.
    let mut results: Vec<Option<AssertResult>> = vec![None; asserts.len()];
    let mut scored: Vec<Scored> = Vec::new();
    // Collected here rather than sniffed back out of the reason strings later:
    // this is the only place that knows structurally *why* an assert errored.
    let mut classes: Vec<ErrorClass> = Vec::new();

    // Local (deterministic) asserts first.
    let mut deferred_indices: Vec<usize> = Vec::new();
    for (i, assert) in asserts.iter().enumerate() {
        if is_local(&assert.kind) {
            let outcome = evaluate_local(assert, output, engine, vars, metrics)
                .expect("local assert yields an outcome");
            scored.push(scored_of(assert, &outcome));
            results[i] = Some(assert_result(
                assert,
                &outcome,
                AssertStatus::from_pass(outcome.passed),
            ));
        } else {
            deferred_indices.push(i);
        }
    }

    // Decide whether the deferred asserts still matter.
    let remaining_weight: f64 = deferred_indices.iter().map(|i| asserts[*i].weight).sum();
    let matters = remaining_can_change_outcome(&scored, remaining_weight, threshold);

    for i in deferred_indices {
        let assert = &asserts[i];
        if !matters {
            results[i] = Some(skipped_result(assert));
            continue;
        }
        match grader {
            Some(g) => match g.grade(assert, output, vars, engine, Some(base_dir)).await {
                Ok(outcome) => {
                    scored.push(scored_of(assert, &outcome));
                    results[i] = Some(assert_result(
                        assert,
                        &outcome,
                        AssertStatus::from_pass(outcome.passed),
                    ));
                }
                // Fail closed: a grader problem is an error, not a plain fail.
                // The class comes off the error itself, so "no grader is
                // configured" and "the grader broke" stay distinguishable all
                // the way to the stored case.
                Err(e) => {
                    classes.push(e.class());
                    results[i] = Some(error_assert(assert, e.to_string()));
                }
            },
            None => {
                // Fail closed: a deferred assert with no grader is an error.
                classes.push(ErrorClass::new(ErrorClass::GRADER_MISSING));
                results[i] = Some(error_assert(
                    assert,
                    format!(
                        "no grader available for '{}' assertions in this run",
                        assert.kind.name().as_str()
                    ),
                ));
            }
        }
    }

    (
        results.into_iter().map(|r| r.unwrap()).collect(),
        scored,
        classes,
    )
}

fn scored_of(assert: &Assert, outcome: &AssertOutcome) -> Scored {
    Scored {
        weight: assert.weight,
        score: outcome.score,
        passed: outcome.passed,
    }
}

/// The assertion's authored definition, for the UI's Input view: the flattened
/// `AssertKind` (its `type` tag plus the type-specific criteria — the `contains`
/// substring, the `llm-rubric` rubric text + threshold, …) as a JSON object,
/// with a `negate: true` entry added when the assertion is negated. `weight` is
/// intentionally omitted (already carried by `AssertResult.weight`). Returns
/// `None` only if the kind fails to serialize, which does not happen in practice
/// (every `AssertKind` variant is an internally-tagged object).
fn assert_criteria(assert: &Assert) -> Option<serde_json::Value> {
    let mut value = serde_json::to_value(&assert.kind).ok()?;
    if assert.negate {
        if let serde_json::Value::Object(map) = &mut value {
            map.insert("negate".to_string(), serde_json::Value::Bool(true));
        }
    }
    Some(value)
}

fn assert_result(assert: &Assert, outcome: &AssertOutcome, status: AssertStatus) -> AssertResult {
    AssertResult {
        kind: assert.kind.name(),
        status,
        score: outcome.score,
        weight: assert.weight,
        reason: outcome.reason.clone(),
        details: outcome.details.clone(),
        criteria: assert_criteria(assert),
        cached: false,
    }
}

fn error_assert(assert: &Assert, reason: String) -> AssertResult {
    AssertResult {
        kind: assert.kind.name(),
        status: AssertStatus::Error,
        score: 0.0,
        weight: assert.weight,
        reason,
        details: None,
        criteria: assert_criteria(assert),
        cached: false,
    }
}

fn skipped_result(assert: &Assert) -> AssertResult {
    AssertResult {
        kind: assert.kind.name(),
        status: AssertStatus::Skipped,
        score: 0.0,
        weight: assert.weight,
        reason: "skipped: outcome already decided".into(),
        details: None,
        criteria: assert_criteria(assert),
        cached: false,
    }
}

/// The case-level `error` message for a case whose assertions errored, or
/// `None` when none did — so it doubles as the "did anything error" predicate.
///
/// Names the first errored assert and quotes its reason. This string is promoted
/// to the server's `cases.error` column and indexed for full-text search, so it
/// has to carry a diagnosis: the constant this replaced ("one or more assertions
/// errored") spent a column and an FTS slot on nothing, while the actual reason
/// sat one field away in [`AssertResult::reason`]. Later errors are reduced to a
/// count — the first one is nearly always the cause, and the rest are visible in
/// the case drawer.
pub(super) fn assert_error_message(results: &[AssertResult]) -> Option<String> {
    let mut errored = results.iter().filter(|a| a.status == AssertStatus::Error);
    let first = errored.next()?;
    let others = errored.count();

    let kind = first.kind.as_str();
    let reason = first.reason.trim();
    let head = if reason.is_empty() {
        format!("{kind} assertion errored")
    } else {
        format!("{kind} assertion errored: {reason}")
    };
    Some(match others {
        0 => head,
        n => format!("{head} (and {n} more errored)"),
    })
}

/// A `latency` assert measures the provider call itself, so a cache hit would
/// score a timing the run never paid. Callers use this to force a cache bypass.
pub(super) fn has_latency_assert(asserts: &[Assert]) -> bool {
    asserts
        .iter()
        .any(|a| matches!(a.kind, AssertKind::Latency { .. }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asserts::AssertName;

    fn result(kind: AssertName, status: AssertStatus, reason: &str) -> AssertResult {
        AssertResult {
            kind,
            status,
            score: 0.0,
            weight: 1.0,
            reason: reason.to_string(),
            details: None,
            criteria: None,
            cached: false,
        }
    }

    #[test]
    fn no_errored_asserts_yields_no_message() {
        let results = [
            result(AssertName::Contains, AssertStatus::Pass, "ok"),
            result(AssertName::Regex, AssertStatus::Fail, "no match"),
        ];
        assert_eq!(assert_error_message(&results), None);
    }

    /// The whole point of the change: the message names the assert and quotes
    /// its reason, instead of the constant that used to occupy the indexed
    /// `cases.error` column on every graded failure.
    #[test]
    fn a_single_errored_assert_names_its_kind_and_reason() {
        let results = [
            result(AssertName::Contains, AssertStatus::Pass, "ok"),
            result(
                AssertName::LlmRubric,
                AssertStatus::Error,
                "grader returned a truncated verdict",
            ),
        ];
        assert_eq!(
            assert_error_message(&results).as_deref(),
            Some("llm-rubric assertion errored: grader returned a truncated verdict")
        );
    }

    #[test]
    fn later_errors_are_reduced_to_a_count() {
        let results = [
            result(AssertName::LlmRubric, AssertStatus::Error, "grader down"),
            result(AssertName::Exec, AssertStatus::Error, "exit 3"),
            result(AssertName::Similar, AssertStatus::Error, "no embeddings"),
        ];
        assert_eq!(
            assert_error_message(&results).as_deref(),
            Some("llm-rubric assertion errored: grader down (and 2 more errored)")
        );
    }

    /// A grader can fail without saying why; the message must still identify
    /// which assert broke rather than trailing an empty colon.
    #[test]
    fn an_empty_reason_still_names_the_assert() {
        let results = [result(AssertName::Exec, AssertStatus::Error, "   ")];
        assert_eq!(
            assert_error_message(&results).as_deref(),
            Some("exec assertion errored")
        );
    }

    /// `Skipped` is short-circuiting, not failure, and must not be reported as
    /// an error — a case whose deferred asserts were skipped still passed.
    #[test]
    fn skipped_asserts_are_not_errors() {
        let results = [
            result(AssertName::Contains, AssertStatus::Pass, "ok"),
            result(AssertName::LlmRubric, AssertStatus::Skipped, "decided"),
        ];
        assert_eq!(assert_error_message(&results), None);
    }
}
