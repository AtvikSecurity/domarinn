//! DTO for `GET /runs/{id}/config`.

use serde::Serialize;
use ts_rs::TS;

use domarinn_core::ids::RunId;

/// `GET /runs/{id}/config` response: the run's config digest and the config
/// snapshot it was produced from, extracted from the stored run blob. Cheap
/// config fetch for the config-drift badge/features without re-downloading the
/// full export.
#[derive(Debug, Clone, Serialize, TS)]
pub struct RunConfigResponse {
    pub run_id: RunId,
    /// The run's `config_digest`. `None` when the stored document has no digest
    /// (or carries the empty-string sentinel).
    pub config_digest: Option<String>,
    /// The run's `config_snapshot`, verbatim. `null` when the document has none.
    #[ts(type = "unknown")]
    pub config: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn run_config_response_matches_todays_wire_shape() {
        let dto = RunConfigResponse {
            run_id: RunId::new("r-1"),
            config_digest: Some("sha256:deadbeef".to_string()),
            config: json!({ "providers": [], "tests": [] }),
        };
        assert_eq!(
            serde_json::to_value(&dto).unwrap(),
            json!({
                "run_id": "r-1",
                "config_digest": "sha256:deadbeef",
                "config": { "providers": [], "tests": [] },
            })
        );
    }

    #[test]
    fn run_config_response_nulls_absent_digest_and_config() {
        // A run whose stored document lacks a digest / snapshot must serialize
        // those as explicit JSON null, not omit the keys (DTO convention).
        let dto = RunConfigResponse {
            run_id: RunId::new("r-2"),
            config_digest: None,
            config: serde_json::Value::Null,
        };
        let v = serde_json::to_value(&dto).unwrap();
        for key in ["config_digest", "config"] {
            assert!(v.get(key).is_some(), "missing key {key}");
            assert!(
                v[key].is_null(),
                "expected {key} to be null, got {:?}",
                v[key]
            );
        }
        assert_eq!(v["run_id"], "r-2");
    }
}
