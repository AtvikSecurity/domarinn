//! Turning per-assert outcomes into a case score and pass/fail status.
//!
//! With a test `threshold`, a case passes when its weighted-mean score meets the
//! threshold. Without one, every assert must pass. This module also decides
//! whether the remaining (expensive) asserts can still change the outcome, which
//! drives short-circuiting of the LLM grader.

/// A scored assert outcome (weight + score in 0..=1 + pass flag).
#[derive(Debug, Clone, Copy)]
pub struct Scored {
    pub weight: f64,
    pub score: f64,
    pub passed: bool,
}

/// The verdict for a whole case.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CaseVerdict {
    pub passed: bool,
    pub score: f64,
}

/// Compute the case verdict from all evaluated asserts.
pub fn case_verdict(scored: &[Scored], threshold: Option<f64>) -> CaseVerdict {
    let score = weighted_mean(scored);
    let passed = match threshold {
        Some(t) => score >= t,
        None => scored.iter().all(|s| s.passed),
    };
    CaseVerdict { passed, score }
}

/// The weighted mean score over asserts (1.0 when there are no asserts).
pub fn weighted_mean(scored: &[Scored]) -> f64 {
    let total_weight: f64 = scored.iter().map(|s| s.weight).sum();
    if total_weight <= 0.0 {
        return 1.0;
    }
    let sum: f64 = scored.iter().map(|s| s.score * s.weight).sum();
    sum / total_weight
}

/// Given the asserts already scored plus the total weight of asserts not yet
/// evaluated, decide whether those remaining asserts could still change the
/// case outcome. If not, the caller may skip them (short-circuit the grader).
///
/// `remaining_weight` is the sum of weights of not-yet-run asserts.
pub fn remaining_can_change_outcome(
    scored_so_far: &[Scored],
    remaining_weight: f64,
    threshold: Option<f64>,
) -> bool {
    if remaining_weight <= 0.0 {
        return false;
    }
    match threshold {
        None => {
            // All-must-pass: if something already failed, the case is already
            // decided (fail) regardless of the rest.
            scored_so_far.iter().all(|s| s.passed)
        }
        Some(t) => {
            let done_weight: f64 = scored_so_far.iter().map(|s| s.weight).sum();
            let done_sum: f64 = scored_so_far.iter().map(|s| s.score * s.weight).sum();
            let total_weight = done_weight + remaining_weight;
            // Best case: remaining all score 1.0. Worst case: remaining all 0.0.
            let best = (done_sum + remaining_weight) / total_weight;
            let worst = done_sum / total_weight;
            // Remaining matters only if the threshold sits between worst and best.
            best >= t && worst < t
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(score: f64, weight: f64) -> Scored {
        Scored {
            weight,
            score,
            passed: score >= 0.5,
        }
    }

    #[test]
    fn all_pass_without_threshold() {
        let v = case_verdict(&[s(1.0, 1.0), s(1.0, 3.0)], None);
        assert!(v.passed);
        assert_eq!(v.score, 1.0);
    }

    #[test]
    fn one_fail_without_threshold_fails() {
        let v = case_verdict(&[s(1.0, 1.0), s(0.0, 3.0)], None);
        assert!(!v.passed);
    }

    #[test]
    fn threshold_uses_weighted_mean() {
        // scores 1.0 (w1) and 0.0 (w3) → mean 0.25.
        let v = case_verdict(&[s(1.0, 1.0), s(0.0, 3.0)], Some(0.75));
        assert!(!v.passed);
        assert!((v.score - 0.25).abs() < 1e-9);
        let v2 = case_verdict(&[s(1.0, 3.0), s(0.0, 1.0)], Some(0.7));
        assert!(v2.passed, "mean 0.75 >= 0.7");
    }

    #[test]
    fn empty_asserts_pass() {
        let v = case_verdict(&[], None);
        assert!(v.passed);
        assert_eq!(v.score, 1.0);
    }

    #[test]
    fn short_circuit_when_already_failed_no_threshold() {
        // A failed deterministic assert with more to come: cannot change (fail).
        assert!(!remaining_can_change_outcome(&[s(0.0, 1.0)], 3.0, None));
        // Still all passing: remaining could still fail it.
        assert!(remaining_can_change_outcome(&[s(1.0, 1.0)], 3.0, None));
    }

    #[test]
    fn short_circuit_when_threshold_unreachable() {
        // done: score 0 weight 1; remaining weight 1; total 2; best 0.5 < 0.75.
        assert!(!remaining_can_change_outcome(
            &[s(0.0, 1.0)],
            1.0,
            Some(0.75)
        ));
        // done: score 0 weight 1; remaining weight 3; best 0.75 >= 0.75 → matters.
        assert!(remaining_can_change_outcome(
            &[s(0.0, 1.0)],
            3.0,
            Some(0.75)
        ));
    }

    #[test]
    fn short_circuit_when_threshold_already_met() {
        // done: score 1 weight 3; remaining weight 1; worst 0.75 >= 0.5 → decided.
        assert!(!remaining_can_change_outcome(
            &[s(1.0, 3.0)],
            1.0,
            Some(0.5)
        ));
    }
}
