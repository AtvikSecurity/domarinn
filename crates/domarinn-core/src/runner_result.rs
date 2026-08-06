//! Shaping a finished (or failed) cell into stored form: the inputs a case
//! knows before a provider answers, the errored-case constructor, the retention
//! rule for bulky provenance, and the run-level rollup.
//!
//! Split out of `runner.rs` — which orchestrates the matrix — so each file stays
//! under the per-file line ratchet (`tests/file_length.rs`), following the seam
//! `runner_asserts.rs` established. Included as a private child module of
//! `runner`, hence the `pub(super)` items.

use serde_json::Value as Json;

use crate::config::TestCase;
use crate::ids::CaseKey;
use crate::result::{CaseResult, CaseStatus, CellKey, RunSummary};
use crate::types::RenderedPrompt;

use super::runner_cache::CallFailure;

/// Upper bound on the raw provider metadata persisted per case. A payload over
/// this size is dropped wholesale (truncated JSON is useless) rather than stored.
pub(super) const RAW_MAX_BYTES: usize = 64 * 1024;

/// Everything a case knows about its own inputs before a provider responds:
/// what was rendered, and what was going to be sent.
///
/// Grouped so `error_case` can hand all of it to a failed case — an errored case
/// that still shows its request is the difference between "HTTP 404" and "HTTP
/// 404, and here is the model id we asked for".
#[derive(Default)]
pub(super) struct CaseInputs {
    pub prompt: Option<RenderedPrompt>,
    pub vars: serde_json::Map<String, serde_json::Value>,
    pub request: Option<Json>,
    /// Identity of what was going to be sent. Present for every failure after
    /// the request was built; `None` for the two earlier ones (rendering vars,
    /// rendering the prompt), which honestly have no input identity yet.
    pub prompt_digest: Option<String>,
    pub provider_digest: Option<String>,
    /// Whoever answered, when a `fallback:` chain was walked and it was not the
    /// provider the cell names. `None` on every path that never reached a call.
    pub answered_by_provider_id: Option<String>,
    /// Chain links tried and passed over. Empty unless a chain ran.
    pub fallback_attempts: Vec<crate::result::FallbackAttempt>,
}

pub(super) fn error_case(
    cell: CellKey,
    case_key: CaseKey,
    name: Option<String>,
    test: &TestCase,
    failure: CallFailure,
    wall_ms: u64,
    inputs: CaseInputs,
) -> CaseResult {
    CaseResult {
        // An errored cell never reached a response, so there is nothing to
        // report — not even an empty claim about what the model decided.
        tool_calls: Vec::new(),
        cache_key: failure.cache_key.map(|k| k.0),
        cell,
        case_key,
        name,
        tags: test.tags.clone(),
        vars: inputs.vars,
        status: CaseStatus::Error,
        score: 0.0,
        output: None,
        prompt: inputs.prompt,
        request: inputs.request,
        stop_reason: None,
        // The call failed, so nothing reported a model.
        model: None,
        raw: None,
        asserts: Vec::new(),
        usage: None,
        cost_usd: None,
        // 0 when the call never reached the provider (a cache-only miss or a
        // cache read error): there is no provider latency to report.
        latency_ms: 0,
        wall_ms: Some(wall_ms),
        cached: false,
        attempts: failure.attempts,
        error_class: Some(failure.class),
        prompt_digest: inputs.prompt_digest,
        provider_digest: inputs.provider_digest,
        // An errored case never graded anything, so there is no assert
        // definition to identify.
        assert_digest: None,
        error: Some(failure.message),
        // Size-capped like `raw`, but not gated on `include_raw`: see the field
        // docs. An errored case has no output and no raw payload, so dropping
        // this would leave nothing but prose.
        error_details: json_to_persist(true, failure.details, "error_details"),
        reasoning: None,
        empty_reason: None,
        answered_by_provider_id: inputs.answered_by_provider_id,
        fallback_attempts: inputs.fallback_attempts,
    }
}

/// Apply the retention policy for a bulky JSON provenance payload: keep it only
/// when raw persistence is enabled and it fits within [`RAW_MAX_BYTES`];
/// otherwise drop it to `None` (an oversized blob is dropped whole — truncated
/// JSON is useless). `what` names the payload in the drop log.
pub(super) fn json_to_persist(include_raw: bool, raw: Option<Json>, what: &str) -> Option<Json> {
    if !include_raw {
        return None;
    }
    let raw = raw?;
    match serde_json::to_vec(&raw) {
        Ok(bytes) if bytes.len() > RAW_MAX_BYTES => {
            tracing::debug!(
                raw_bytes = bytes.len(),
                max_bytes = RAW_MAX_BYTES,
                payload = what,
                "dropping oversized provider payload"
            );
            None
        }
        Ok(_) => Some(raw),
        Err(e) => {
            tracing::debug!(error = %e, payload = what, "dropping unserializable provider payload");
            None
        }
    }
}

pub(super) fn summarize(cases: &[CaseResult]) -> RunSummary {
    let mut s = RunSummary {
        total: cases.len() as u64,
        ..Default::default()
    };
    // Integer accumulation: a float sum over thousands of tiny per-case costs
    // makes the total depend on the order it was added in, so the same run
    // re-summed can disagree with itself. Converted back to USD once, at the
    // end, where the wire type is a float.
    let mut cost = crate::pricing::MicroUsd::ZERO;
    let mut saved = crate::pricing::MicroUsd::ZERO;
    let mut grader_cost = crate::pricing::MicroUsd::ZERO;
    let mut any_cost = false;
    let mut any_grader_cost = false;
    for c in cases {
        match c.status {
            CaseStatus::Pass => s.passed += 1,
            CaseStatus::Fail => s.failed += 1,
            CaseStatus::Error => s.errored += 1,
            CaseStatus::Skip => s.skipped += 1,
        }
        if let Some(u) = &c.usage {
            // Cache reads included: they are prompt tokens that were sent, and
            // the providers report them in their own field rather than in
            // `input_tokens`. Counting only the latter reports a 6,000-token
            // prompt as 200 the moment the provider's cache warms.
            s.prompt_tokens += u
                .input_tokens
                .saturating_add(u.cache_read_tokens.unwrap_or(0));
            s.completion_tokens += u.output_tokens;
            s.cache_read_tokens += u.cache_read_tokens.unwrap_or(0);
            s.cache_write_tokens += u.cache_write_tokens.unwrap_or(0);
        }
        if let Some(cst) = c.cost_usd {
            let micro = crate::pricing::MicroUsd::from_usd(cst);
            cost = cost.saturating_add(micro);
            // A cached case still reports what the work cost; it just did not
            // cost that again today. Summing those is the saving, exactly,
            // with no re-pricing and no dependence on the current rate table.
            if c.cached {
                saved = saved.saturating_add(micro);
            }
            any_cost = true;
        }
        // Judge cost, kept in its own accumulator rather than added to `cost`.
        // A `cost:` assertion budgets the system under test, and a run's
        // headline cost is what that system cost — grading it is a separate
        // line item, often the larger one when the judge is the bigger model.
        for a in &c.asserts {
            if let Some(cst) = a.cost_usd {
                grader_cost = grader_cost.saturating_add(crate::pricing::MicroUsd::from_usd(cst));
                any_grader_cost = true;
            }
        }
        if c.cached {
            s.cache_hits += 1;
        } else {
            s.cache_misses += 1;
        }
        // A cached case replays the original attempt count, so counting it here
        // would re-report a retry that happened on some earlier run.
        if !c.cached && c.attempts > 1 {
            s.retried_cases += 1;
        }
        // Keyed on the field being present, which is set only when the answering
        // provider differed from the configured one — so a chain that was
        // configured and never needed does not count. Skipped cases do not
        // count either, whoever answered them: this tally feeds the CLI's "all
        // graded cases were answered by a fallback" gate, and a skipped case
        // graded nothing.
        if c.status != CaseStatus::Skip && c.answered_by_provider_id.is_some() {
            s.fallback_cases += 1;
        }
        // Keyed on the reason string, not a variant: the set is open, so an
        // unknown vendor reason has to tally under its own name rather than
        // collapse into a catch-all. An empty-string reason is excluded: no
        // provider path produces one (the exec parser filters it at the seam),
        // but a hand-authored document can carry it, and `''` is the "known:
        // not empty" sentinel every server-side tally already excludes —
        // counting it here would render a nameless `( × 1)` entry and make the
        // CLI's footer disagree with the server about the same document.
        if let Some(reason) = c.empty_reason.as_ref().filter(|r| !r.as_str().is_empty()) {
            *s.empty_counts
                .entry(reason.as_str().to_string())
                .or_default() += 1;
        }
    }
    s.cost_usd = any_cost.then(|| cost.to_usd());
    s.cache_savings_usd =
        (any_cost && saved > crate::pricing::MicroUsd::ZERO).then(|| saved.to_usd());
    s.grader_cost_usd = any_grader_cost.then(|| grader_cost.to_usd());
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::empty::EmptyReason;

    fn case(test_id: &str, status: CaseStatus, empty_reason: Option<&str>) -> CaseResult {
        CaseResult {
            cache_key: None,
            answered_by_provider_id: None,
            fallback_attempts: Vec::new(),
            tool_calls: Vec::new(),
            cell: CellKey {
                provider_id: "p".into(),
                prompt_id: None,
                test_id: test_id.into(),
                repeat: 0,
            },
            case_key: test_id.into(),
            name: Some(test_id.into()),
            tags: vec![],
            vars: Default::default(),
            status,
            score: 0.0,
            output: None,
            prompt: None,
            request: None,
            stop_reason: None,
            raw: None,
            asserts: vec![],
            usage: None,
            cost_usd: None,
            latency_ms: 0,
            wall_ms: None,
            reasoning: None,
            empty_reason: empty_reason.map(EmptyReason::new),
            cached: false,
            attempts: 1,
            prompt_digest: None,
            provider_digest: None,
            assert_digest: None,
            error: None,
            error_details: None,
            model: None,
            error_class: None,
        }
    }

    /// The run-level answer to "how many of these refused?", which previously
    /// existed only per case and had to be recounted by every reader.
    ///
    /// The statuses are load-bearing, not incidental. The tally keys on
    /// `empty_reason` alone, across every status — narrowing it to graded cases
    /// would drop exactly the cases the feature reports on. A `Skip` is the
    /// clearest: `skip_on_empty_reason` skips a case *because* of its reason, so
    /// a skipped case always carries one. An `Error` can too, since an errored
    /// assert sets the status while the classified reason rides along
    /// (`runner.rs`); only a provider failure (`error_case`) never has one.
    #[test]
    fn summarize_tallies_empty_counts_by_reason() {
        let cases = vec![
            case("a", CaseStatus::Fail, Some(EmptyReason::REFUSAL)),
            case("b", CaseStatus::Skip, Some(EmptyReason::REFUSAL)),
            case("c", CaseStatus::Error, Some(EmptyReason::TRUNCATED)),
        ];

        let summary = summarize(&cases);

        assert_eq!(
            summary.empty_counts,
            std::collections::BTreeMap::from([
                (EmptyReason::REFUSAL.to_string(), 2),
                (EmptyReason::TRUNCATED.to_string(), 1),
            ])
        );
    }

    #[test]
    fn summarize_leaves_empty_counts_empty_when_no_case_is_empty() {
        let summary = summarize(&[case("a", CaseStatus::Pass, None)]);
        assert!(summary.empty_counts.is_empty());
    }

    /// `""` is the storage layer's "known: not empty" sentinel, and the server
    /// excludes it from every tally. A hand-authored document carrying one must
    /// not make the CLI's footer say "1 empty" (with a nameless markdown row)
    /// for a run the server reports as having none.
    #[test]
    fn summarize_does_not_tally_an_empty_string_reason() {
        let summary = summarize(&[case("a", CaseStatus::Fail, Some(""))]);
        assert!(summary.empty_counts.is_empty());
    }
}
