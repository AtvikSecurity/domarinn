//! Diffing two runs: per-case transitions plus a significance test.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::ids::CaseKey;
use crate::result::{CaseStatus, RunResult};
use crate::stats::{mcnemar, McNemar};

/// How a single case changed between two runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum Delta {
    NewlyFailing,
    NewlyPassing,
    StillFailing,
    Unchanged,
    Added,
    Removed,
}

// The change axis lives in `domarinn-types`: it is a wire value carried on
// every compare row. Re-exported so `domarinn_core::diff::CaseChange` and
// friends keep resolving for existing callers.
pub use domarinn_types::change::{classify_change, CaseChange, ChangeInputs};

/// A per-case delta record.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct CaseDelta {
    pub case_key: CaseKey,
    pub name: Option<String>,
    pub base_status: Option<CaseStatus>,
    pub head_status: Option<CaseStatus>,
    pub delta: Delta,
    pub output_changed: bool,
}

/// Aggregate counts for a run diff.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
pub struct DiffSummary {
    pub newly_failing: u64,
    pub newly_passing: u64,
    pub still_failing: u64,
    pub unchanged: u64,
    pub output_changed: u64,
    pub added: u64,
    pub removed: u64,
}

/// The full diff between two runs.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct RunDiff {
    pub cases: Vec<CaseDelta>,
    pub summary: DiffSummary,
    pub mcnemar: McNemarView,
}

/// A serializable view of the McNemar result.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct McNemarView {
    pub regressions: u64,
    pub fixes: u64,
    pub statistic: f64,
    pub significant: bool,
}

impl From<McNemar> for McNemarView {
    fn from(m: McNemar) -> Self {
        McNemarView {
            regressions: m.b,
            fixes: m.c,
            statistic: m.statistic,
            significant: m.significant,
        }
    }
}

impl RunDiff {
    /// Whether the head run regressed relative to base (any newly-failing case).
    pub fn has_regression(&self) -> bool {
        self.summary.newly_failing > 0
    }
}

/// The diff axis measures the **model**, not the gate: an `XPass` is the model
/// genuinely passing (the failing gate is the stale `expect_fail` marker — a
/// config problem the exit code reports separately), and an `XFail` is the
/// model failing, stably, so a mature suite's known-failing set never reads as
/// a regression.
fn passed(status: CaseStatus) -> bool {
    matches!(status, CaseStatus::Pass | CaseStatus::XPass)
}

/// Compute the diff of `head` against `base`, joining on `case_key`.
pub fn diff_runs(base: &RunResult, head: &RunResult) -> RunDiff {
    let base_by_key: BTreeMap<&CaseKey, &crate::result::CaseResult> =
        base.cases.iter().map(|c| (&c.case_key, c)).collect();
    let head_by_key: BTreeMap<&CaseKey, &crate::result::CaseResult> =
        head.cases.iter().map(|c| (&c.case_key, c)).collect();

    let mut cases = Vec::new();
    let mut summary = DiffSummary::default();
    let (mut b_count, mut c_count) = (0u64, 0u64);

    // Cases present in head (some may be new).
    for hc in &head.cases {
        let base = base_by_key.get(&hc.case_key);
        let head_pass = passed(hc.status);
        let (delta, base_status, output_changed) = match base {
            Some(bc) => {
                let base_pass = passed(bc.status);
                let output_changed = output_text(bc) != output_text(hc);
                let delta = match (base_pass, head_pass) {
                    (true, false) => {
                        b_count += 1;
                        Delta::NewlyFailing
                    }
                    (false, true) => {
                        c_count += 1;
                        Delta::NewlyPassing
                    }
                    (false, false) => Delta::StillFailing,
                    (true, true) => Delta::Unchanged,
                };
                (delta, Some(bc.status), output_changed)
            }
            None => (Delta::Added, None, false),
        };
        tally(&mut summary, delta, output_changed);
        cases.push(CaseDelta {
            case_key: hc.case_key.clone(),
            name: hc.name.clone(),
            base_status,
            head_status: Some(hc.status),
            delta,
            output_changed,
        });
    }

    // Cases only in base (removed).
    for bc in &base.cases {
        if !head_by_key.contains_key(&bc.case_key) {
            summary.removed += 1;
            cases.push(CaseDelta {
                case_key: bc.case_key.clone(),
                name: bc.name.clone(),
                base_status: Some(bc.status),
                head_status: None,
                delta: Delta::Removed,
                output_changed: false,
            });
        }
    }

    RunDiff {
        cases,
        summary,
        mcnemar: mcnemar(b_count, c_count).into(),
    }
}

fn tally(summary: &mut DiffSummary, delta: Delta, output_changed: bool) {
    match delta {
        Delta::NewlyFailing => summary.newly_failing += 1,
        Delta::NewlyPassing => summary.newly_passing += 1,
        Delta::StillFailing => summary.still_failing += 1,
        Delta::Unchanged => summary.unchanged += 1,
        Delta::Added => summary.added += 1,
        Delta::Removed => summary.removed += 1,
    }
    if output_changed {
        summary.output_changed += 1;
    }
}

fn output_text(case: &crate::result::CaseResult) -> String {
    case.output
        .as_ref()
        .map(|o| o.as_text().into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::result::{CaseResult, CellKey, RunSummary};
    use crate::types::Output;
    use chrono::Utc;

    fn case(key: &str, status: CaseStatus, output: &str) -> CaseResult {
        CaseResult {
            cache_key: None,
            tool_calls: Vec::new(),
            cell: CellKey {
                provider_id: "p".into(),
                prompt_id: None,
                test_id: key.into(),
                repeat: 0,
            },
            case_key: key.into(),
            name: Some(key.into()),
            tags: vec![],
            vars: Default::default(),
            status,
            score: if matches!(status, CaseStatus::Pass) {
                1.0
            } else {
                0.0
            },
            output: Some(Output::Text(output.into())),
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
            empty_reason: None,
            cached: false,
            attempts: 1,
            prompt_digest: None,
            provider_digest: None,
            assert_digest: None,
            error: None,
            error_details: None,
            model: None,
            error_class: None,
            answered_by_provider_id: None,
            fallback_attempts: Vec::new(),
            expect_fail_reason: None,
        }
    }

    fn run(cases: Vec<CaseResult>) -> RunResult {
        RunResult {
            schema_version: 1,
            run_id: "r".into(),
            project: None,
            suite: None,
            started_at: Utc::now(),
            finished_at: Utc::now(),
            config_digest: "d".into(),
            config_snapshot: serde_json::Value::Null,
            git: None,
            ci: None,
            digests: None,
            origin: None,
            share_url: None,
            composite: None,
            filters: Default::default(),
            summary: RunSummary::default(),
            cases,
        }
    }

    /// The diff axis measures the *model*, the exit code measures the *gate*.
    /// `passed()` therefore counts XPass as passing (the model genuinely
    /// passed; the failing gate is the stale annotation, which the exit code
    /// already reports) and XFail as failing — stably, so a suite of known
    /// failures never reads as regressing.
    #[test]
    fn expected_failure_transitions_measure_the_model_not_the_gate() {
        let base = run(vec![
            case("stable-xfail", CaseStatus::XFail, "x"),
            case("fixed-under-marker", CaseStatus::Fail, "x"),
            case("marker-added", CaseStatus::Pass, "x"),
            case("fixed-and-unmarked", CaseStatus::XFail, "x"),
        ]);
        let head = run(vec![
            // XFail→XFail: the normal shape of a mature suite. Not a change.
            case("stable-xfail", CaseStatus::XFail, "x"),
            // Fail→XPass: the model improved; report NewlyPassing.
            case("fixed-under-marker", CaseStatus::XPass, "x"),
            // Pass→XFail: the model regressed (annotated in the same window,
            // but the diff still reports the model's movement).
            case("marker-added", CaseStatus::XFail, "x"),
            // XFail→Pass: fixed and the marker removed; NewlyPassing.
            case("fixed-and-unmarked", CaseStatus::Pass, "x"),
        ]);
        let d = diff_runs(&base, &head);
        assert_eq!(d.summary.newly_passing, 2);
        assert_eq!(d.summary.newly_failing, 1);
        assert_eq!(d.summary.still_failing, 1);
        assert_eq!(d.summary.unchanged, 0);
        // McNemar counts stay honest model-behavior counts.
        assert_eq!(d.mcnemar.fixes, 2);
        assert_eq!(d.mcnemar.regressions, 1);
    }

    /// A suite whose xfail set is stable has no regression to report.
    #[test]
    fn a_stable_xfail_set_is_not_a_regression() {
        let base = run(vec![case("known", CaseStatus::XFail, "x")]);
        let head = run(vec![case("known", CaseStatus::XFail, "x")]);
        assert!(!diff_runs(&base, &head).has_regression());
    }

    #[test]
    fn classifies_all_transitions() {
        let base = run(vec![
            case("keep", CaseStatus::Pass, "x"),
            case("regress", CaseStatus::Pass, "x"),
            case("fix", CaseStatus::Fail, "x"),
            case("still", CaseStatus::Fail, "x"),
            case("gone", CaseStatus::Pass, "x"),
        ]);
        let head = run(vec![
            case("keep", CaseStatus::Pass, "x"),
            case("regress", CaseStatus::Fail, "x"),
            case("fix", CaseStatus::Pass, "x"),
            case("still", CaseStatus::Fail, "y"), // output changed
            case("new", CaseStatus::Pass, "x"),
        ]);
        let d = diff_runs(&base, &head);
        assert_eq!(d.summary.newly_failing, 1);
        assert_eq!(d.summary.newly_passing, 1);
        assert_eq!(d.summary.still_failing, 1);
        assert_eq!(d.summary.unchanged, 1);
        assert_eq!(d.summary.added, 1);
        assert_eq!(d.summary.removed, 1);
        assert_eq!(d.summary.output_changed, 1);
        assert!(d.has_regression());
        assert_eq!(d.mcnemar.regressions, 1);
        assert_eq!(d.mcnemar.fixes, 1);
    }

    #[test]
    fn no_regression_when_only_fixes() {
        let base = run(vec![case("a", CaseStatus::Fail, "x")]);
        let head = run(vec![case("a", CaseStatus::Pass, "x")]);
        let d = diff_runs(&base, &head);
        assert!(!d.has_regression());
        assert_eq!(d.summary.newly_passing, 1);
    }
}
