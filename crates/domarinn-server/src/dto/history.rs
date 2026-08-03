//! DTOs for `GET /projects/{project}/suites/{suite}/cases/{case_key}/history`.
//!
//! One case's evolution across the most recent runs of a suite: the same
//! `case_key` (a deterministic hash of provider|prompt|test|repeat) identifies
//! the same logical matrix cell in every run, so this endpoint walks that cell
//! backwards in time — status, score, output-hash, and latency — powering the
//! UI's per-case "History" timeline and baseline-drift view.

use serde::Serialize;
use ts_rs::TS;

use domarinn_core::ids::{CaseKey, RunId};
use domarinn_core::result::CaseStatus;

/// `GET /projects/{project}/suites/{suite}/cases/{case_key}/history` response.
///
/// `points` are newest-first (the query's `ORDER BY created_at DESC, id DESC`).
#[derive(Debug, Clone, Serialize, TS)]
pub struct CaseHistoryResponse {
    pub project: String,
    pub suite: String,
    pub case_key: CaseKey,
    /// The suite's baseline run, when one is set; `None` otherwise.
    pub baseline_run_id: Option<RunId>,
    pub points: Vec<CaseHistoryPoint>,
}

/// One run's snapshot of the case, newest first within
/// [`CaseHistoryResponse::points`].
#[derive(Debug, Clone, Serialize, TS)]
pub struct CaseHistoryPoint {
    pub run_id: RunId,
    /// RFC3339.
    pub created_at: String,
    pub status: CaseStatus,
    pub score: Option<f64>,
    pub output_hash: Option<String>,
    /// Whether this run's output differs from the chronologically previous
    /// run's (i.e. the next-older point, `points[i + 1]`). `None` for the oldest
    /// returned point and whenever either side's `output_hash` is NULL.
    pub output_changed: Option<bool>,
    /// Whether this run's response for the case came from the provider cache
    /// (the migration-6 `cases.cached` column).
    ///
    /// `None` means unknown, not fresh: legacy pre-backfill rows are NULL and
    /// undecodable blobs carry the `-1` sentinel, and neither may be reported
    /// as `false` — that would claim a measurement nobody made.
    ///
    /// The timeline uses this to collapse a run of consecutive cached points
    /// into one marker rather than hiding them. Hiding would misreport how
    /// long a verdict held; a replayed result is still evidence the case was
    /// green on that date, just weaker evidence than a fresh call.
    ///
    /// Note this is deliberately *not* a filter: no history point is ever
    /// dropped, so `output_changed` keeps comparing genuinely adjacent runs.
    pub cached: Option<bool>,
    pub prompt_tokens: Option<i64>,
    pub completion_tokens: Option<i64>,
    pub cost_usd: Option<f64>,
    pub latency_ms: Option<i64>,
    pub git_commit: Option<String>,
    pub config_digest: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn case_history_response_matches_todays_wire_shape() {
        let dto = CaseHistoryResponse {
            project: "proj".to_string(),
            suite: "suite".to_string(),
            case_key: CaseKey::new("deadbeef"),
            baseline_run_id: Some(RunId::new("r-1")),
            points: vec![CaseHistoryPoint {
                run_id: RunId::new("r-2"),
                created_at: "2026-01-01T00:00:00+00:00".to_string(),
                status: CaseStatus::Pass,
                score: Some(1.0),
                output_hash: Some("abc123".to_string()),
                output_changed: Some(true),
                cached: Some(false),
                prompt_tokens: Some(10),
                completion_tokens: Some(20),
                cost_usd: Some(0.0025),
                latency_ms: Some(42),
                git_commit: Some("abc123".to_string()),
                config_digest: Some("sha256:deadbeef".to_string()),
            }],
        };
        assert_eq!(
            serde_json::to_value(&dto).unwrap(),
            json!({
                "project": "proj",
                "suite": "suite",
                "case_key": "deadbeef",
                "baseline_run_id": "r-1",
                "points": [
                    {
                        "run_id": "r-2",
                        "created_at": "2026-01-01T00:00:00+00:00",
                        "status": "pass",
                        "score": 1.0,
                        "output_hash": "abc123",
                        "output_changed": true,
                        "cached": false,
                        "prompt_tokens": 10,
                        "completion_tokens": 20,
                        "cost_usd": 0.0025,
                        "latency_ms": 42,
                        "git_commit": "abc123",
                        "config_digest": "sha256:deadbeef",
                    }
                ],
            })
        );
    }

    #[test]
    fn case_history_optionals_serialize_as_null_not_omitted() {
        let dto = CaseHistoryResponse {
            project: "proj".to_string(),
            suite: "suite".to_string(),
            case_key: CaseKey::new("deadbeef"),
            baseline_run_id: None,
            points: vec![CaseHistoryPoint {
                run_id: RunId::new("r-1"),
                created_at: "2026-01-01T00:00:00+00:00".to_string(),
                status: CaseStatus::Skip,
                score: None,
                output_hash: None,
                output_changed: None,
                cached: None,
                prompt_tokens: None,
                completion_tokens: None,
                cost_usd: None,
                latency_ms: None,
                git_commit: None,
                config_digest: None,
            }],
        };
        let v = serde_json::to_value(&dto).unwrap();
        // The suite may have no baseline; the key is still present as `null`.
        assert!(v.get("baseline_run_id").is_some());
        assert!(v["baseline_run_id"].is_null());
        assert_eq!(v["points"].as_array().unwrap().len(), 1);
        let p = &v["points"][0];
        assert_eq!(p["status"], "skip");
        // Every optional serializes as explicit `null`, never omitted, so the
        // UI always sees the keys.
        for key in [
            "score",
            "output_hash",
            "output_changed",
            "cached",
            "prompt_tokens",
            "completion_tokens",
            "cost_usd",
            "latency_ms",
            "git_commit",
            "config_digest",
        ] {
            assert!(p.get(key).is_some(), "missing key {key}");
            assert!(
                p[key].is_null(),
                "expected {key} to be null, got {:?}",
                p[key]
            );
        }
    }
}
