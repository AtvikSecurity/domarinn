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
        *counts.entry(class_or_unknown(c).to_string()).or_default() += 1;
    }
    counts
}

/// The one spelling of "which bucket does this case's class land in".
///
/// `None` and `""` are the same absence — `ErrorClass` is an unvalidated
/// transparent string, and an exec child can (and did) emit an empty class.
/// This rule used to live twice: here with the empty-string filter and in the
/// errored-cases table without it, so one PR comment said `unknown × 1` in its
/// metric row beside a blank Class cell for the very same case.
pub(crate) fn class_or_unknown(case: &CaseResult) -> &str {
    case.error_class
        .as_ref()
        .map(|k| k.as_str())
        .filter(|k| !k.is_empty())
        .unwrap_or(UNKNOWN_CLASS)
}

/// Every distinct class an errored case recorded, collapsed-first.
///
/// The case-level `error_class` is a *collapse*: when asserts of both gate
/// tiers error on one case, `most_specific` keeps only the harness class so
/// the exit code is right — and the suite fault would otherwise vanish from
/// every report. The per-assert rows still carry it, so the errored-cases
/// table lists them all: the collapsed class first (it explains the exit
/// code), then any others in stable order.
pub(crate) fn case_classes(case: &CaseResult) -> String {
    let primary = class_or_unknown(case);
    let mut out = vec![primary.to_string()];
    let mut extras: Vec<&str> = case
        .asserts
        .iter()
        .filter_map(|a| a.error_class.as_ref())
        .map(|k| k.as_str())
        .filter(|k| !k.is_empty() && *k != primary)
        .collect();
    extras.sort_unstable();
    extras.dedup();
    out.extend(extras.iter().map(|k| k.to_string()));
    out.join(", ")
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

    /// One case, two tiers: the case-level class is a collapse (harness wins,
    /// so the exit code is right), and the suite fault it drops survives only
    /// on the per-assert rows. `case_classes` is what puts it back in front of
    /// a reader — collapsed class first, the rest in stable order.
    #[test]
    fn case_classes_lists_the_classes_the_collapse_dropped() {
        use domarinn_core::asserts::AssertName;
        use domarinn_core::result::{AssertResult, AssertStatus};

        fn errored_assert(class: &str) -> AssertResult {
            AssertResult {
                kind: AssertName::Exec,
                status: AssertStatus::Error,
                score: 0.0,
                weight: 1.0,
                reason: "boom".into(),
                details: None,
                criteria: None,
                cached: false,
                cost_usd: None,
                error_class: Some(ErrorClass::new(class)),
            }
        }

        let mut c = case(CaseStatus::Error, Some("exec_failed"));
        c.asserts.push(errored_assert("grader_missing"));
        assert_eq!(case_classes(&c), "exec_failed, grader_missing");

        // A lone class renders bare — no trailing separator, no duplicate when
        // the assert row repeats the collapsed class.
        let mut c = case(CaseStatus::Error, Some("grader_failed"));
        c.asserts.push(errored_assert("grader_failed"));
        assert_eq!(case_classes(&c), "grader_failed");

        // No class anywhere is still the unknown bucket.
        let c = case(CaseStatus::Error, None);
        assert_eq!(case_classes(&c), "unknown");
    }
}
