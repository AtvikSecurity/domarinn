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
) -> (Vec<AssertResult>, Vec<Scored>) {
    // Slot results by original index so output order matches config order.
    let mut results: Vec<Option<AssertResult>> = vec![None; asserts.len()];
    let mut scored: Vec<Scored> = Vec::new();

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
                Err(reason) => {
                    results[i] = Some(error_assert(assert, reason));
                }
            },
            None => {
                // Fail closed: a deferred assert with no grader is an error.
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

    (results.into_iter().map(|r| r.unwrap()).collect(), scored)
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

impl AssertStatus {
    fn from_pass(pass: bool) -> AssertStatus {
        if pass {
            AssertStatus::Pass
        } else {
            AssertStatus::Fail
        }
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
