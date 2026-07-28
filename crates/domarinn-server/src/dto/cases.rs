//! DTOs for `GET /runs/{id}/cases` and `GET /runs/{id}/cases/{case_key}`.

use serde::Serialize;
use ts_rs::TS;

use domarinn_core::ids::CaseKey;
use domarinn_core::result::CaseStatus;

use super::runs::CaseAssertLean;

/// One row of `GET /runs/{id}/cases` — lean: a preview and the assert
/// outcomes, not the full stored `CaseResult` (see [`CaseDetailResponse`]).
#[derive(Debug, Clone, Serialize, TS)]
pub struct CaseListItem {
    pub case_key: CaseKey,
    pub idx: i64,
    pub name: Option<String>,
    pub status: CaseStatus,
    pub output_preview: Option<String>,
    pub asserts: Vec<CaseAssertLean>,
    pub prompt_tokens: Option<i64>,
    pub completion_tokens: Option<i64>,
    pub cost_usd: Option<f64>,
    pub latency_ms: Option<i64>,
    /// Matrix-cell identity, promoted out of the stored blob (migration 3) so
    /// the UI can filter/pivot without decompressing each case. All optional:
    /// legacy/pre-backfill rows are NULL and failed-backfill rows carry the
    /// empty-string sentinel, which the list query maps to `None`.
    pub provider_id: Option<String>,
    pub prompt_id: Option<String>,
    pub test_id: Option<String>,
    /// DB column is `repeat_idx`; the wire name is `repeat`.
    pub repeat: Option<i64>,
    pub score: Option<f64>,
    pub stop_reason: Option<String>,
    /// Whether the provider response was a cache hit (migration-6 `cases`
    /// column). `None` for legacy pre-backfill rows and failed-backfill rows
    /// carrying the -1 sentinel, which the list query maps to `None`.
    pub cached: Option<bool>,
    /// The failure reason for an errored case (migration-7 `cases` column).
    /// `output_preview` derives from `output`, which an errored case does not
    /// have, so without this the grid can only show that a row errored and not
    /// why. `None` for a case that did not error, and for legacy pre-backfill
    /// rows.
    pub error: Option<String>,
    /// What kind of failure `error` describes (migration-10 `cases` column).
    /// Lets a run's errors be grouped — `provider_*` is not the model's fault,
    /// `grader_*` means the eval did not run — instead of read one at a time.
    /// `None` for a case that did not error and for rows written before this.
    pub error_class: Option<String>,
}

/// `GET /runs/{id}/cases` response.
#[derive(Debug, Clone, Serialize, TS)]
pub struct CaseListResponse {
    pub cases: Vec<CaseListItem>,
    pub next_cursor: Option<String>,
}

/// `GET /runs/{id}/cases/{case_key}` response: the decompressed, stored
/// `CaseResult` document, returned verbatim (forward-compatible; not
/// re-derived into a typed shape here, per the storage module's contract).
#[derive(Debug, Clone, Serialize, TS)]
#[serde(transparent)]
pub struct CaseDetailResponse(#[ts(type = "unknown")] pub serde_json::Value);

#[cfg(test)]
mod tests {
    use super::*;
    use domarinn_core::asserts::AssertName;
    use serde_json::json;

    #[test]
    fn case_list_item_matches_todays_wire_shape() {
        let dto = CaseListItem {
            case_key: CaseKey::new("deadbeef"),
            idx: 0,
            name: Some("openai::t1".to_string()),
            status: CaseStatus::Pass,
            output_preview: Some("hello".to_string()),
            asserts: vec![CaseAssertLean {
                label: AssertName::Contains,
                kind: AssertName::Contains,
                passed: true,
                score: 1.0,
            }],
            prompt_tokens: Some(10),
            completion_tokens: Some(20),
            cost_usd: Some(0.0025),
            latency_ms: Some(42),
            provider_id: Some("openai".to_string()),
            prompt_id: Some("default".to_string()),
            test_id: Some("t1".to_string()),
            repeat: Some(0),
            score: Some(1.0),
            stop_reason: Some("stop".to_string()),
            cached: Some(true),
            error: None,
            error_class: None,
        };
        assert_eq!(
            serde_json::to_value(&dto).unwrap(),
            json!({
                "case_key": "deadbeef",
                "idx": 0,
                "name": "openai::t1",
                "status": "pass",
                "output_preview": "hello",
                "asserts": [
                    { "label": "contains", "kind": "contains", "passed": true, "score": 1.0 }
                ],
                "prompt_tokens": 10,
                "completion_tokens": 20,
                "cost_usd": 0.0025,
                "latency_ms": 42,
                "provider_id": "openai",
                "prompt_id": "default",
                "test_id": "t1",
                "repeat": 0,
                "score": 1.0,
                "stop_reason": "stop",
                "cached": true,
                "error": null,
                "error_class": null,
            })
        );
    }

    #[test]
    fn case_list_item_asserts_is_always_an_array_never_null() {
        let dto = CaseListItem {
            case_key: CaseKey::new("deadbeef"),
            idx: 0,
            name: None,
            status: CaseStatus::Skip,
            output_preview: None,
            asserts: vec![],
            prompt_tokens: None,
            completion_tokens: None,
            cost_usd: None,
            latency_ms: None,
            provider_id: None,
            prompt_id: None,
            test_id: None,
            repeat: None,
            score: None,
            stop_reason: None,
            cached: None,
            error: None,
            error_class: None,
        };
        let v = serde_json::to_value(&dto).unwrap();
        assert_eq!(v["asserts"], json!([]));
        assert!(v["cached"].is_null());
        assert!(v["name"].is_null());
        assert_eq!(v["status"], "skip");
        // The migration-3 cell fields serialize as explicit `null`, never
        // omitted (DTO null-not-omitted convention), so the UI always sees the
        // keys.
        for key in [
            "provider_id",
            "prompt_id",
            "test_id",
            "repeat",
            "score",
            "stop_reason",
        ] {
            assert!(v.get(key).is_some(), "missing key {key}");
            assert!(
                v[key].is_null(),
                "expected {key} to be null, got {:?}",
                v[key]
            );
        }
    }

    #[test]
    fn case_detail_response_serializes_the_wrapped_value_verbatim() {
        let inner = json!({ "case_key": "deadbeef", "output": "hello world" });
        let dto = CaseDetailResponse(inner.clone());
        assert_eq!(serde_json::to_value(&dto).unwrap(), inner);
    }
}
