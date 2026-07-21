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

use domarinn_core::ids::{CaseKey, RunId};
use domarinn_core::result::CaseStatus;

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

/// `GET /runs/{id}/compare/{other}` response.
#[derive(Debug, Clone, Serialize, TS)]
pub struct CompareResponse {
    pub base: RunId,
    pub head: RunId,
    pub summary: CompareSummary,
    pub cases: Vec<CompareCaseRow>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
            }],
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
                    }
                ],
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
    fn added_case_has_null_base_status() {
        let row = CompareCaseRow {
            case_key: CaseKey::new("x"),
            name: Some("t5".to_string()),
            base_status: None,
            head_status: Some(CaseStatus::Pass),
            delta: CompareDelta::Added,
            output_changed: false,
        };
        let v = serde_json::to_value(&row).unwrap();
        assert!(v["base_status"].is_null());
        assert_eq!(v["delta"], "added");
    }
}
