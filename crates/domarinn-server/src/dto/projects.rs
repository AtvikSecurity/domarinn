//! DTOs for `GET /projects` and `GET /projects/{project}/suites`.

use serde::Serialize;
use ts_rs::TS;

use domarinn_core::ids::RunId;

/// One row of `GET /projects`.
#[derive(Debug, Clone, Serialize, TS)]
pub struct ProjectListItem {
    pub project: String,
    pub run_count: i64,
    pub suite_count: i64,
    /// RFC3339.
    pub last_run_at: String,
}

/// `GET /projects` response.
#[derive(Debug, Clone, Serialize, TS)]
pub struct ProjectsResponse {
    pub projects: Vec<ProjectListItem>,
}

/// One point in a suite's recent pass-rate series (newest first, capped at
/// 20 runs).
#[derive(Debug, Clone, Serialize, TS)]
pub struct SuitePoint {
    pub run_id: RunId,
    /// RFC3339.
    pub created_at: String,
    pub total: i64,
    pub passed: i64,
    pub pass_rate: f64,
}

/// One suite's summary within `GET /projects/{project}/suites`. Not named
/// `SuiteSummaryDto` — `SuiteSummary` does not collide with any core-exported
/// TS type (core exports `RunSummary`, not `SuiteSummary`).
#[derive(Debug, Clone, Serialize, TS)]
pub struct SuiteSummary {
    pub suite: String,
    pub run_count: i64,
    /// RFC3339. `None` only if the suite somehow has zero runs in its series
    /// (the suite name itself always comes from at least one row).
    pub last_run_at: Option<String>,
    pub baseline_run_id: Option<RunId>,
    pub series: Vec<SuitePoint>,
}

/// `GET /projects/{project}/suites` response.
#[derive(Debug, Clone, Serialize, TS)]
pub struct SuitesResponse {
    pub project: String,
    pub suites: Vec<SuiteSummary>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn projects_response_matches_todays_wire_shape() {
        let dto = ProjectsResponse {
            projects: vec![ProjectListItem {
                project: "proj".to_string(),
                run_count: 2,
                suite_count: 1,
                last_run_at: "2026-01-01T00:00:00+00:00".to_string(),
            }],
        };
        assert_eq!(
            serde_json::to_value(&dto).unwrap(),
            json!({
                "projects": [
                    {
                        "project": "proj",
                        "run_count": 2,
                        "suite_count": 1,
                        "last_run_at": "2026-01-01T00:00:00+00:00",
                    }
                ]
            })
        );
    }

    #[test]
    fn suites_response_matches_todays_wire_shape() {
        let dto = SuitesResponse {
            project: "proj".to_string(),
            suites: vec![SuiteSummary {
                suite: "suite".to_string(),
                run_count: 1,
                last_run_at: Some("2026-01-01T00:00:00+00:00".to_string()),
                baseline_run_id: Some(RunId::new("p-1")),
                series: vec![SuitePoint {
                    run_id: RunId::new("p-2"),
                    created_at: "2026-01-01T00:00:00+00:00".to_string(),
                    total: 2,
                    passed: 2,
                    pass_rate: 1.0,
                }],
            }],
        };
        assert_eq!(
            serde_json::to_value(&dto).unwrap(),
            json!({
                "project": "proj",
                "suites": [
                    {
                        "suite": "suite",
                        "run_count": 1,
                        "last_run_at": "2026-01-01T00:00:00+00:00",
                        "baseline_run_id": "p-1",
                        "series": [
                            {
                                "run_id": "p-2",
                                "created_at": "2026-01-01T00:00:00+00:00",
                                "total": 2,
                                "passed": 2,
                                "pass_rate": 1.0,
                            }
                        ],
                    }
                ]
            })
        );
    }

    #[test]
    fn suite_with_no_baseline_serializes_null_not_omitted() {
        let dto = SuiteSummary {
            suite: "suite".to_string(),
            run_count: 0,
            last_run_at: None,
            baseline_run_id: None,
            series: vec![],
        };
        let v = serde_json::to_value(&dto).unwrap();
        assert!(v.get("baseline_run_id").is_some());
        assert!(v["baseline_run_id"].is_null());
        assert!(v["last_run_at"].is_null());
        assert_eq!(v["series"], json!([]));
    }
}
