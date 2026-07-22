//! DTOs for `GET /runs/{id}/compare/{other}`.
//!
//! [`CompareDelta`] is a *local* enum, not core's [`domarinn_core::diff::Delta`]:
//! the two disagree on which cases are "unchanged". Core's `diff::Delta`
//! folds every same-status pair (pass/pass *and* e.g. skip/skip) into a
//! single `Unchanged` variant. This endpoint instead special-cases pass/pass
//! as `still_passing` and only falls back to `unchanged` for same-status
//! pairs that are neither pass/pass nor fail-or-error/fail-or-error (in
//! practice: a `skip` on either side). Reusing core's `Delta` here would
//! silently change the wire format (today's `still_passing` cases would
//! become `unchanged`), so this task keeps the endpoint's own strings —
//! see `storage::compare` for the classification this type mirrors.

use serde::Serialize;
use ts_rs::TS;

use domarinn_core::asserts::AssertName;
use domarinn_core::diff::McNemarView;
use domarinn_core::ids::{CaseKey, RunId};
use domarinn_core::result::CaseStatus;
use domarinn_core::stats::PassRate;

/// How a case's status changed between the base and head runs, in this
/// endpoint's own classification (see the module doc for why this isn't
/// core's `diff::Delta`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum CompareDelta {
    NewlyFailing,
    NewlyPassing,
    StillFailing,
    StillPassing,
    Unchanged,
    Added,
    Removed,
}

/// One case's row in a compare response.
#[derive(Debug, Clone, Serialize, TS)]
pub struct CompareCaseRow {
    pub case_key: CaseKey,
    pub name: Option<String>,
    pub base_status: Option<CaseStatus>,
    pub head_status: Option<CaseStatus>,
    pub delta: CompareDelta,
    pub output_changed: bool,
    /// The case's overall score in the base run (column-only; `None` when the
    /// case is absent from base or the row carries no score).
    pub base_score: Option<f64>,
    /// The case's overall score in the head run.
    pub head_score: Option<f64>,
    /// `head_score - base_score`, only when *both* are present.
    pub score_delta: Option<f64>,
    /// Assertions whose pass/fail flipped between base and head for this case;
    /// empty when nothing flipped or the case exists on only one side. See
    /// [`AssertFlip`] for the (necessarily heuristic) pairing.
    pub assert_flips: Vec<AssertFlip>,
}

/// A single assertion whose pass/fail flipped between the base and head runs
/// for one case.
///
/// The lean assert records this is computed from carry no stable per-assert id,
/// so base and head asserts are paired heuristically (see `storage::compare`):
/// positionally when the two runs' kind-sequences are identical, otherwise by
/// kind for kinds that occur exactly once on both sides. Asserts that can't be
/// paired that way contribute no flip.
#[derive(Debug, Clone, Serialize, TS)]
pub struct AssertFlip {
    pub kind: AssertName,
    pub base_passed: bool,
    pub head_passed: bool,
    pub base_score: f64,
    pub head_score: f64,
}

/// Aggregate counts for a compare response. Notably does *not* tally
/// `still_passing` or `unchanged` cases — matches today's endpoint, which
/// only counts the six categories below.
#[derive(Debug, Clone, Serialize, TS)]
pub struct CompareSummary {
    pub newly_failing: u64,
    pub newly_passing: u64,
    pub still_failing: u64,
    pub output_changed: u64,
    pub added: u64,
    pub removed: u64,
}

/// Significance statistics for a compare: a McNemar paired test over the
/// status transitions plus a Wilson pass-rate interval for each run.
#[derive(Debug, Clone, Serialize, TS)]
pub struct CompareStats {
    pub mcnemar: McNemarView,
    pub base_pass_rate: WilsonView,
    pub head_pass_rate: WilsonView,
}

/// A serializable view of a Wilson-interval pass rate. Mirrors
/// [`domarinn_core::stats::PassRate`], which is not itself `Serialize`.
#[derive(Debug, Clone, Serialize, TS)]
pub struct WilsonView {
    pub passed: i64,
    pub total: i64,
    pub rate: f64,
    pub lower: f64,
    pub upper: f64,
}

impl From<PassRate> for WilsonView {
    fn from(p: PassRate) -> Self {
        WilsonView {
            passed: p.passed as i64,
            total: p.total as i64,
            rate: p.rate,
            lower: p.lower,
            upper: p.upper,
        }
    }
}

/// Per-run aggregate totals for a compare (from the `runs` table columns; no
/// blob loads).
#[derive(Debug, Clone, Serialize, TS)]
pub struct CompareTotals {
    pub base: RunTotals,
    pub head: RunTotals,
}

/// Aggregate totals for one run.
#[derive(Debug, Clone, Serialize, TS)]
pub struct RunTotals {
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub cost_usd: Option<f64>,
    pub duration_ms: Option<i64>,
    pub case_count: i64,
    pub pass_count: i64,
}

/// Config-digest drift between the two runs. `changed` is `None` unless both
/// digests are known.
#[derive(Debug, Clone, Serialize, TS)]
pub struct CompareConfig {
    pub base_digest: Option<String>,
    pub head_digest: Option<String>,
    pub changed: Option<bool>,
}

/// `GET /runs/{id}/compare/{other}` response.
#[derive(Debug, Clone, Serialize, TS)]
pub struct CompareResponse {
    pub base: RunId,
    pub head: RunId,
    pub summary: CompareSummary,
    pub cases: Vec<CompareCaseRow>,
    pub stats: CompareStats,
    pub totals: CompareTotals,
    pub config: CompareConfig,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_stats() -> CompareStats {
        CompareStats {
            mcnemar: McNemarView {
                regressions: 1,
                fixes: 1,
                statistic: 0.0,
                significant: false,
            },
            base_pass_rate: WilsonView {
                passed: 3,
                total: 4,
                rate: 0.75,
                lower: 0.3,
                upper: 0.95,
            },
            head_pass_rate: WilsonView {
                passed: 2,
                total: 4,
                rate: 0.5,
                lower: 0.15,
                upper: 0.85,
            },
        }
    }

    fn sample_totals() -> CompareTotals {
        CompareTotals {
            base: RunTotals {
                prompt_tokens: 40,
                completion_tokens: 80,
                cost_usd: Some(0.01),
                duration_ms: Some(30_000),
                case_count: 4,
                pass_count: 3,
            },
            head: RunTotals {
                prompt_tokens: 40,
                completion_tokens: 80,
                cost_usd: Some(0.01),
                duration_ms: Some(30_000),
                case_count: 4,
                pass_count: 2,
            },
        }
    }

    #[test]
    fn compare_response_matches_todays_wire_shape() {
        let dto = CompareResponse {
            base: RunId::new("base"),
            head: RunId::new("head"),
            summary: CompareSummary {
                newly_failing: 1,
                newly_passing: 1,
                still_failing: 0,
                output_changed: 1,
                added: 1,
                removed: 1,
            },
            cases: vec![CompareCaseRow {
                case_key: CaseKey::new("deadbeef"),
                name: Some("t1".to_string()),
                base_status: Some(CaseStatus::Pass),
                head_status: Some(CaseStatus::Fail),
                delta: CompareDelta::NewlyFailing,
                output_changed: false,
                base_score: Some(1.0),
                head_score: Some(0.0),
                score_delta: Some(-1.0),
                assert_flips: vec![AssertFlip {
                    kind: AssertName::Contains,
                    base_passed: true,
                    head_passed: false,
                    base_score: 1.0,
                    head_score: 0.0,
                }],
            }],
            stats: sample_stats(),
            totals: sample_totals(),
            config: CompareConfig {
                base_digest: Some("sha256:aaa".to_string()),
                head_digest: Some("sha256:bbb".to_string()),
                changed: Some(true),
            },
        };
        assert_eq!(
            serde_json::to_value(&dto).unwrap(),
            json!({
                "base": "base",
                "head": "head",
                "summary": {
                    "newly_failing": 1,
                    "newly_passing": 1,
                    "still_failing": 0,
                    "output_changed": 1,
                    "added": 1,
                    "removed": 1,
                },
                "cases": [
                    {
                        "case_key": "deadbeef",
                        "name": "t1",
                        "base_status": "pass",
                        "head_status": "fail",
                        "delta": "newly_failing",
                        "output_changed": false,
                        "base_score": 1.0,
                        "head_score": 0.0,
                        "score_delta": -1.0,
                        "assert_flips": [
                            {
                                "kind": "contains",
                                "base_passed": true,
                                "head_passed": false,
                                "base_score": 1.0,
                                "head_score": 0.0,
                            }
                        ],
                    }
                ],
                "stats": {
                    "mcnemar": {
                        "regressions": 1,
                        "fixes": 1,
                        "statistic": 0.0,
                        "significant": false,
                    },
                    "base_pass_rate": {
                        "passed": 3,
                        "total": 4,
                        "rate": 0.75,
                        "lower": 0.3,
                        "upper": 0.95,
                    },
                    "head_pass_rate": {
                        "passed": 2,
                        "total": 4,
                        "rate": 0.5,
                        "lower": 0.15,
                        "upper": 0.85,
                    },
                },
                "totals": {
                    "base": {
                        "prompt_tokens": 40,
                        "completion_tokens": 80,
                        "cost_usd": 0.01,
                        "duration_ms": 30000,
                        "case_count": 4,
                        "pass_count": 3,
                    },
                    "head": {
                        "prompt_tokens": 40,
                        "completion_tokens": 80,
                        "cost_usd": 0.01,
                        "duration_ms": 30000,
                        "case_count": 4,
                        "pass_count": 2,
                    },
                },
                "config": {
                    "base_digest": "sha256:aaa",
                    "head_digest": "sha256:bbb",
                    "changed": true,
                },
            })
        );
    }

    #[test]
    fn compare_delta_still_passing_and_added_removed_serialize_as_todays_strings() {
        assert_eq!(
            serde_json::to_value(CompareDelta::StillPassing).unwrap(),
            json!("still_passing")
        );
        assert_eq!(
            serde_json::to_value(CompareDelta::Unchanged).unwrap(),
            json!("unchanged")
        );
        assert_eq!(
            serde_json::to_value(CompareDelta::Added).unwrap(),
            json!("added")
        );
        assert_eq!(
            serde_json::to_value(CompareDelta::Removed).unwrap(),
            json!("removed")
        );
    }

    #[test]
    fn added_case_has_null_base_status_and_score() {
        let row = CompareCaseRow {
            case_key: CaseKey::new("x"),
            name: Some("t5".to_string()),
            base_status: None,
            head_status: Some(CaseStatus::Pass),
            delta: CompareDelta::Added,
            output_changed: false,
            base_score: None,
            head_score: Some(1.0),
            score_delta: None,
            assert_flips: vec![],
        };
        let v = serde_json::to_value(&row).unwrap();
        assert!(v["base_status"].is_null());
        assert_eq!(v["delta"], "added");
        // Optionals serialize as explicit `null`, and empty flips as `[]`.
        assert!(v["base_score"].is_null());
        assert!(v["score_delta"].is_null());
        assert_eq!(v["head_score"], 1.0);
        assert_eq!(v["assert_flips"], json!([]));
    }

    #[test]
    fn wilson_view_is_a_clean_from_pass_rate() {
        let pr = PassRate {
            passed: 7,
            total: 10,
            rate: 0.7,
            lower: 0.4,
            upper: 0.9,
        };
        let view: WilsonView = pr.into();
        assert_eq!(
            serde_json::to_value(&view).unwrap(),
            json!({
                "passed": 7,
                "total": 10,
                "rate": 0.7,
                "lower": 0.4,
                "upper": 0.9,
            })
        );
    }

    #[test]
    fn compare_config_changed_is_null_unless_both_digests_known() {
        let cfg = CompareConfig {
            base_digest: Some("sha256:aaa".to_string()),
            head_digest: None,
            changed: None,
        };
        let v = serde_json::to_value(&cfg).unwrap();
        assert_eq!(v["base_digest"], "sha256:aaa");
        assert!(v["head_digest"].is_null());
        assert!(v["changed"].is_null());
    }

    #[test]
    fn run_totals_optionals_serialize_as_null() {
        let totals = RunTotals {
            prompt_tokens: 0,
            completion_tokens: 0,
            cost_usd: None,
            duration_ms: None,
            case_count: 0,
            pass_count: 0,
        };
        let v = serde_json::to_value(&totals).unwrap();
        assert!(v["cost_usd"].is_null());
        assert!(v["duration_ms"].is_null());
        assert_eq!(v["case_count"], 0);
    }
}
