//! A run's errored cases, tallied by [`ErrorClass`].
//!
//! A gate that says "2 errored" and stops has told a reader the run is red and
//! nothing else. The classes group by *owner*: `grader_failed` means the judge
//! never produced a usable verdict, `provider_timeout` means the call never
//! landed, `grader_missing` means the suite is wrong. Those go to three
//! different people, and the count alone picks none of them.
//!
//! Derived from the cases rather than from a counter on [`RunSummary`], for the
//! same reason as [`crate::output::graded_fallback_cases`]: it is then correct
//! for a stored document of any vintage, and it needs no new always-emitted
//! field — one of those would shift the content hash the server keys
//! idempotency on and turn a re-upload into a `409`.

use std::collections::BTreeMap;

use domarinn_core::result::{CaseResult, CaseStatus};

/// The bucket for an errored case carrying no class.
///
/// Spelled the same as the web UI's `aggregateErrorClasses`, which has bucketed
/// these under `unknown` since it was written. Two names for the same bucket
/// would make the PR comment and the run page disagree about a run neither can
/// classify.
pub(crate) const UNKNOWN_CLASS: &str = "unknown";

/// How many errored cases hit each class, keyed by class name.
///
/// `BTreeMap` so the order is the same on every run rather than following
/// whatever order the providers happened to fail in — the same stability rule
/// the `Empty` row's `empty_counts` relies on.
pub(crate) fn error_class_counts(cases: &[CaseResult]) -> BTreeMap<String, u64> {
    let mut counts: BTreeMap<String, u64> = BTreeMap::new();
    for c in cases.iter().filter(|c| c.status == CaseStatus::Error) {
        let key = c
            .error_class
            .as_ref()
            .map(|k| k.as_str())
            .filter(|k| !k.is_empty())
            .unwrap_or(UNKNOWN_CLASS);
        *counts.entry(key.to_string()).or_default() += 1;
    }
    counts
}

/// The tally as one line: `grader_failed × 2, provider_timeout × 1`.
///
/// Empty when nothing errored, so a caller can treat "" as "no row to print"
/// without also consulting the count. Shared by the markdown metric row and the
/// action's `error-classes` step output precisely so the PR comment and the
/// job's error annotation cannot describe the same run differently.
pub(crate) fn error_class_summary(cases: &[CaseResult]) -> String {
    error_class_counts(cases)
        .iter()
        .map(|(class, count)| format!("{class} × {count}"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use domarinn_core::error_class::ErrorClass;

    fn case(status: CaseStatus, class: Option<&str>) -> CaseResult {
        let mut c = crate::output::sample_run().cases.remove(0);
        c.status = status;
        c.error = (status == CaseStatus::Error).then(|| "boom".to_string());
        c.error_class = class.map(ErrorClass::new);
        c
    }

    #[test]
    fn a_clean_run_tallies_nothing() {
        let cases = [case(CaseStatus::Pass, None), case(CaseStatus::Fail, None)];
        assert!(error_class_counts(&cases).is_empty());
        assert_eq!(error_class_summary(&cases), "");
    }

    /// Only `Error` cases count. A *failed* case is a judgement about the model
    /// and belongs nowhere near an error-class tally — counting it would make
    /// the breakdown disagree with the run's own `errored` total.
    #[test]
    fn only_errored_cases_are_tallied() {
        let cases = [
            case(CaseStatus::Fail, None),
            case(CaseStatus::Error, Some("grader_failed")),
            case(CaseStatus::Pass, None),
        ];
        assert_eq!(error_class_summary(&cases), "grader_failed × 1");
    }

    #[test]
    fn classes_are_counted_and_rendered_in_a_stable_order() {
        let cases = [
            case(CaseStatus::Error, Some("provider_timeout")),
            case(CaseStatus::Error, Some("grader_failed")),
            case(CaseStatus::Error, Some("grader_failed")),
        ];
        let counts = error_class_counts(&cases);
        assert_eq!(counts.get("grader_failed"), Some(&2));
        assert_eq!(counts.get("provider_timeout"), Some(&1));
        // Alphabetical, not first-seen: the same run must render identically
        // every time it is summarized.
        assert_eq!(
            error_class_summary(&cases),
            "grader_failed × 2, provider_timeout × 1"
        );
    }

    /// Runs stored before `error_class` existed carry prose and no class. They
    /// are bucketed, never dropped, so the breakdown still adds up to the
    /// run's error count and nobody is left wondering where the rest went.
    #[test]
    fn an_errored_case_without_a_class_is_bucketed_not_dropped() {
        let cases = [
            case(CaseStatus::Error, None),
            case(CaseStatus::Error, Some("grader_failed")),
        ];
        assert_eq!(
            error_class_summary(&cases),
            "grader_failed × 1, unknown × 1"
        );
        assert_eq!(
            error_class_counts(&cases).values().sum::<u64>(),
            2,
            "the tally must account for every errored case"
        );
    }

    /// An empty-string class is the same absence as `None` and must not render
    /// a nameless `( × 1)` entry — the rule `empty_counts` already applies to
    /// its own reasons.
    #[test]
    fn a_blank_class_is_treated_as_unknown() {
        let cases = [case(CaseStatus::Error, Some(""))];
        assert_eq!(error_class_summary(&cases), "unknown × 1");
    }
}
