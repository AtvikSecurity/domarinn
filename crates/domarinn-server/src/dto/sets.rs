//! DTOs for the run-set browser and its access lists (`/api/v1/sets*`).
//!
//! # Two different meanings of "restricted"
//!
//! The *browse* views ([`ProjectSetView`], [`SuiteSetView`],
//! [`SuiteSetDetailResponse`]) answer "is this set locked for me right now", so
//! their `restricted` is the **covering** answer: a project-level restriction
//! makes every suite under it restricted.
//!
//! [`SetAccessResponse::restricted`] is the **exact-scope** answer, because that
//! is the row the restriction toggle beside it creates and deletes. A suite
//! inside a restricted project therefore reports `false` on its own access
//! panel — its restriction lives on the project, and that is where it is
//! removed. This mirrors [`crate::storage::Storage::list_run_set_grants`], which
//! is exact-scope for the same reason.
//!
//! # `last_run_at` is epoch-ms
//!
//! Unlike the legacy [`crate::dto::projects`] DTOs, which emit RFC3339 strings,
//! every timestamp here is integer epoch-ms — the same unit `created_at` uses on
//! grants, so the browser formats one kind of value.
//!
//! # `empty_count` is a lower bound, and omitted when there is nothing to say
//!
//! Every other count here is a plain `i64` summed over the set's runs.
//! `empty_count` is not, because `runs.empty_count` is tri-state: a legacy row
//! that predates migration 15 is NULL, and one whose blob would not decode
//! carries `-1`. Neither is zero, so neither is summed — such runs contribute
//! nothing, which makes the total a **lower bound** on a corpus that still has
//! un-backfilled runs.
//!
//! The field is then omitted, not zeroed, when that total is `0` — the same
//! rule [`crate::dto::runs::RunListItem::empty_count`] follows, so "absent"
//! means "nothing to report" on both the run row and the set row it rolls up
//! into. A reader must render absence as blank, never as `0`.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use domarinn_core::ids::RunId;

use crate::domain::UserId;
use crate::runsets::GrantLevel;

/// One project's row in `GET /sets`, aggregated over every run of it the caller
/// may see.
#[derive(Debug, Clone, Serialize, TS)]
pub struct ProjectSetView {
    pub project: String,
    pub suite_count: i64,
    pub run_count: i64,
    /// Epoch-ms. `None` only for a project with no visible runs, which the
    /// listing does not produce.
    pub last_run_at: Option<i64>,
    pub pass_count: i64,
    pub fail_count: i64,
    pub error_count: i64,
    pub case_count: i64,
    /// Empty-output cases across the set's runs — see this module's header.
    #[ts(optional)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub empty_count: Option<u64>,
    /// One latest pass rate per suite, suites in name order, capped — enough
    /// for a sparkline of the project's spread, not a time series.
    pub recent_pass_rates: Vec<f64>,
    /// Whether a project-level restriction covers the whole project.
    pub restricted: bool,
    /// The grant this caller holds over the project. `None` for callers that do
    /// not ride grants at all (admins, anonymous and static-token callers) and
    /// for a user who simply holds none.
    pub my_level: Option<GrantLevel>,
}

/// `GET /sets` response. Sorted by project name; never paginated (project
/// cardinality is small by construction).
#[derive(Debug, Clone, Serialize, TS)]
pub struct SetsResponse {
    pub projects: Vec<ProjectSetView>,
}

/// One suite's row within `GET /sets/{project}`.
#[derive(Debug, Clone, Serialize, TS)]
pub struct SuiteSetView {
    pub suite: String,
    pub run_count: i64,
    /// Epoch-ms.
    pub last_run_at: Option<i64>,
    pub pass_count: i64,
    pub fail_count: i64,
    pub error_count: i64,
    pub case_count: i64,
    /// Empty-output cases across the set's runs — see this module's header.
    #[ts(optional)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub empty_count: Option<u64>,
    /// The newest run's pass rate — the last element of `sparkline`.
    pub latest_pass_rate: Option<f64>,
    /// The last 20 runs' pass rates, **oldest first** — the order the web
    /// `Sparkline` component consumes.
    pub sparkline: Vec<f64>,
    pub baseline_run_id: Option<RunId>,
    /// Covering: true when either the suite or its whole project is restricted.
    pub restricted: bool,
    pub my_level: Option<GrantLevel>,
}

/// `GET /sets/{project}` response.
#[derive(Debug, Clone, Serialize, TS)]
pub struct ProjectSetDetailResponse {
    pub project: String,
    pub restricted: bool,
    pub my_level: Option<GrantLevel>,
    pub suites: Vec<SuiteSetView>,
}

/// `GET /sets/{project}/suites/{suite}` response. The same aggregates as the
/// suite's row in the project detail, addressed directly.
#[derive(Debug, Clone, Serialize, TS)]
pub struct SuiteSetDetailResponse {
    pub project: String,
    pub suite: String,
    pub restricted: bool,
    pub my_level: Option<GrantLevel>,
    pub run_count: i64,
    /// Epoch-ms.
    pub last_run_at: Option<i64>,
    pub pass_count: i64,
    pub fail_count: i64,
    pub error_count: i64,
    pub case_count: i64,
    /// Empty-output cases across the set's runs — see this module's header.
    #[ts(optional)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub empty_count: Option<u64>,
    pub latest_pass_rate: Option<f64>,
    /// The last 20 runs' pass rates, oldest first — see `SuiteSetView`.
    pub sparkline: Vec<f64>,
    pub baseline_run_id: Option<RunId>,
}

/// One row of a set's access list.
#[derive(Debug, Clone, Serialize, TS)]
pub struct SetGrantView {
    pub user_id: UserId,
    pub username: String,
    pub level: GrantLevel,
    /// Epoch-ms.
    pub created_at: i64,
    /// The identity label that recorded the grant, by the same convention as
    /// `runs.uploaded_by`. `None` for grants written without one.
    pub created_by: Option<String>,
}

/// `GET /sets/{project}/access` (and the suite variant) response.
#[derive(Debug, Clone, Serialize, TS)]
pub struct SetAccessResponse {
    /// Exact scope — see this module's header.
    pub restricted: bool,
    pub grants: Vec<SetGrantView>,
}

/// `PUT /sets/{project}/grants/{user_id}` (and the suite variant) request body.
#[derive(Debug, Clone, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct SetGrantUpsert {
    pub level: GrantLevel,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sets_response_matches_todays_wire_shape() {
        let dto = SetsResponse {
            projects: vec![ProjectSetView {
                project: "checkout".to_string(),
                suite_count: 2,
                run_count: 4,
                last_run_at: Some(1_767_225_600_000),
                pass_count: 4,
                fail_count: 3,
                error_count: 0,
                case_count: 7,
                empty_count: Some(2),
                recent_pass_rates: vec![0.5, 1.0],
                restricted: true,
                my_level: Some(GrantLevel::Manage),
            }],
        };
        assert_eq!(
            serde_json::to_value(&dto).unwrap(),
            json!({
                "projects": [
                    {
                        "project": "checkout",
                        "suite_count": 2,
                        "run_count": 4,
                        "last_run_at": 1767225600000_i64,
                        "pass_count": 4,
                        "fail_count": 3,
                        "error_count": 0,
                        "case_count": 7,
                        "empty_count": 2,
                        "recent_pass_rates": [0.5, 1.0],
                        "restricted": true,
                        "my_level": "manage",
                    }
                ]
            })
        );
    }

    #[test]
    fn a_set_with_nothing_to_report_nulls_its_optionals() {
        let dto = ProjectSetView {
            project: "empty".to_string(),
            suite_count: 0,
            run_count: 0,
            last_run_at: None,
            pass_count: 0,
            fail_count: 0,
            error_count: 0,
            case_count: 0,
            empty_count: None,
            recent_pass_rates: vec![],
            restricted: false,
            my_level: None,
        };
        let v = serde_json::to_value(&dto).unwrap();
        for key in ["last_run_at", "my_level"] {
            assert!(v.get(key).is_some(), "missing key {key}");
            assert!(v[key].is_null(), "expected {key} null, got {:?}", v[key]);
        }
        assert_eq!(v["recent_pass_rates"], json!([]));
        // `empty_count` is the one field here that is omitted rather than
        // nulled when it has nothing to say — see this module's header.
        assert!(
            v.get("empty_count").is_none(),
            "empty_count must be omitted, not null: {v}"
        );
    }

    #[test]
    fn project_set_detail_matches_todays_wire_shape() {
        let dto = ProjectSetDetailResponse {
            project: "checkout".to_string(),
            restricted: false,
            my_level: None,
            suites: vec![SuiteSetView {
                suite: "smoke".to_string(),
                run_count: 2,
                last_run_at: Some(1_767_225_600_000),
                pass_count: 3,
                fail_count: 1,
                error_count: 0,
                case_count: 4,
                empty_count: Some(1),
                latest_pass_rate: Some(1.0),
                sparkline: vec![0.5, 1.0],
                baseline_run_id: Some(RunId::new("r-1")),
                restricted: true,
                my_level: Some(GrantLevel::View),
            }],
        };
        assert_eq!(
            serde_json::to_value(&dto).unwrap(),
            json!({
                "project": "checkout",
                "restricted": false,
                "my_level": null,
                "suites": [
                    {
                        "suite": "smoke",
                        "run_count": 2,
                        "last_run_at": 1767225600000_i64,
                        "pass_count": 3,
                        "fail_count": 1,
                        "error_count": 0,
                        "case_count": 4,
                        "empty_count": 1,
                        "latest_pass_rate": 1.0,
                        "sparkline": [0.5, 1.0],
                        "baseline_run_id": "r-1",
                        "restricted": true,
                        "my_level": "view",
                    }
                ]
            })
        );
    }

    #[test]
    fn suite_set_detail_matches_todays_wire_shape() {
        let dto = SuiteSetDetailResponse {
            project: "checkout".to_string(),
            suite: "smoke".to_string(),
            restricted: false,
            my_level: None,
            run_count: 0,
            last_run_at: None,
            pass_count: 0,
            fail_count: 0,
            error_count: 0,
            case_count: 0,
            empty_count: None,
            latest_pass_rate: None,
            sparkline: vec![],
            baseline_run_id: None,
        };
        assert_eq!(
            serde_json::to_value(&dto).unwrap(),
            json!({
                "project": "checkout",
                "suite": "smoke",
                "restricted": false,
                "my_level": null,
                "run_count": 0,
                "last_run_at": null,
                "pass_count": 0,
                "fail_count": 0,
                "error_count": 0,
                "case_count": 0,
                // No `empty_count`: the key is absent, not null, when the set
                // has nothing to report.
                "latest_pass_rate": null,
                "sparkline": [],
                "baseline_run_id": null,
            })
        );
    }

    #[test]
    fn set_access_response_matches_todays_wire_shape() {
        let dto = SetAccessResponse {
            restricted: true,
            grants: vec![SetGrantView {
                user_id: UserId::new("usr_1"),
                username: "alice".to_string(),
                level: GrantLevel::Upload,
                created_at: 1_767_225_600_000,
                created_by: None,
            }],
        };
        assert_eq!(
            serde_json::to_value(&dto).unwrap(),
            json!({
                "restricted": true,
                "grants": [
                    {
                        "user_id": "usr_1",
                        "username": "alice",
                        "level": "upload",
                        "created_at": 1767225600000_i64,
                        "created_by": null,
                    }
                ]
            })
        );
    }

    #[test]
    fn set_grant_upsert_reads_todays_wire_shape() {
        let body: SetGrantUpsert = serde_json::from_value(json!({ "level": "manage" })).unwrap();
        assert_eq!(body.level, GrantLevel::Manage);
        assert!(
            serde_json::from_value::<SetGrantUpsert>(json!({ "level": "manage", "x": 1 })).is_err(),
            "unknown fields are rejected, like every other request body"
        );
        assert!(serde_json::from_value::<SetGrantUpsert>(json!({ "level": "owner" })).is_err());
    }
}
