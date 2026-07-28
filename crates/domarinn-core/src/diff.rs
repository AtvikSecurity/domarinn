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

/// *What* moved between two runs of one case.
///
/// Orthogonal to [`Delta`], which says whether the **verdict** moved. That
/// distinction is the whole point: "you changed the prompt" and "the model
/// regressed" both surface as `NewlyFailing` today, and they call for opposite
/// responses.
///
/// A closed enum, unlike [`crate::empty::EmptyReason`] and
/// [`crate::error_class::ErrorClass`]: those carry values invented by vendors
/// or by an out-of-tree `exec` child, whereas these are derived by domarinn from
/// digest equality and no third party can add one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum CaseChange {
    /// A digest is absent on one side or the other — one of the runs predates
    /// the field, and `provider_digest` in particular can never be backfilled.
    /// Deliberately its own variant: "unknown" and "unchanged" are different
    /// answers, and collapsing them would invent a finding.
    Unknown,
    /// The request changed — prompt, vars or params. A different output is
    /// expected here, not a finding.
    PromptChanged,
    /// The model or its settings changed.
    ProviderChanged,
    /// The grading definition changed. The goalposts moved, not the system.
    AssertsChanged,
    /// Same request and grading, different output, and the verdict flipped.
    ModelDrift,
    /// Same request and grading, different output, verdict held —
    /// nondeterminism within tolerance.
    OutputDrift,
    /// Same request, same output, same grading — and the verdict flipped
    /// anyway. Only one thing is left that could have moved: the grader.
    ///
    /// This is the diagnosis the digests exist to make possible. Grader
    /// verdicts are never cached (see [`crate::runner::AssertGrader`]), so an
    /// `llm-rubric` suite re-grades on every run even at a 100% provider cache
    /// hit rate — and without this, a flaky grader is indistinguishable from a
    /// real regression.
    UnstableGrader,
    /// Nothing moved.
    Stable,
}

/// The digests and outcomes of one case on both sides of a comparison.
#[derive(Debug, Clone, Copy, Default)]
pub struct ChangeInputs<'a> {
    pub base_prompt: Option<&'a str>,
    pub head_prompt: Option<&'a str>,
    pub base_provider: Option<&'a str>,
    pub head_provider: Option<&'a str>,
    pub base_asserts: Option<&'a str>,
    pub head_asserts: Option<&'a str>,
    pub output_changed: bool,
    pub verdict_changed: bool,
}

/// `Some(true)`/`Some(false)` when both sides are known, `None` when either is
/// missing. Three states, because two would force a guess.
fn moved(base: Option<&str>, head: Option<&str>) -> Option<bool> {
    match (base, head) {
        (Some(b), Some(h)) => Some(b != h),
        _ => None,
    }
}

/// Classify what moved. See [`CaseChange`].
///
/// A definite change on any axis wins over an unknown on another: if the prompt
/// demonstrably changed, it does not matter that the provider digest is
/// unavailable — the finding is already explained.
pub fn classify_change(i: &ChangeInputs) -> CaseChange {
    let prompt = moved(i.base_prompt, i.head_prompt);
    let provider = moved(i.base_provider, i.head_provider);
    let asserts = moved(i.base_asserts, i.head_asserts);

    if prompt == Some(true) {
        return CaseChange::PromptChanged;
    }
    if provider == Some(true) {
        return CaseChange::ProviderChanged;
    }
    if asserts == Some(true) {
        return CaseChange::AssertsChanged;
    }
    // Nothing is known to have changed. Only claim the input held if it is
    // actually known to have held on every axis.
    if prompt.is_none() || provider.is_none() || asserts.is_none() {
        return CaseChange::Unknown;
    }
    match (i.output_changed, i.verdict_changed) {
        (true, true) => CaseChange::ModelDrift,
        (true, false) => CaseChange::OutputDrift,
        (false, true) => CaseChange::UnstableGrader,
        (false, false) => CaseChange::Stable,
    }
}

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

fn passed(status: CaseStatus) -> bool {
    matches!(status, CaseStatus::Pass)
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
            filters: Default::default(),
            summary: RunSummary::default(),
            cases,
        }
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

#[cfg(test)]
#[path = "diff_change_tests.rs"]
mod change_tests;
