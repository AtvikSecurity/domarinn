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
        };
        let v = serde_json::to_value(&dto).unwrap();
        assert_eq!(v["asserts"], json!([]));
        assert!(v["name"].is_null());
        assert_eq!(v["status"], "skip");
    }

    #[test]
    fn case_detail_response_serializes_the_wrapped_value_verbatim() {
        let inner = json!({ "case_key": "deadbeef", "output": "hello world" });
        let dto = CaseDetailResponse(inner.clone());
        assert_eq!(serde_json::to_value(&dto).unwrap(), inner);
    }
}
