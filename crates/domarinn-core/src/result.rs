//! The versioned [`RunResult`] document — the seam between engine, CLI, and
//! server. Bump [`RESULT_SCHEMA_VERSION`] on any breaking change; golden
//! snapshots guard it.

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use ts_rs::TS;

use crate::asserts::AssertName;
use crate::ids::{CaseKey, RunId};
use crate::types::{Output, RenderedPrompt, TokenUsage};

pub const RESULT_SCHEMA_VERSION: u32 = 2;

/// Identity of one cell in the provider × prompt × test × repeat matrix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[ts(optional_fields)]
pub struct CellKey {
    pub provider_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_id: Option<String>,
    pub test_id: String,
    #[serde(default)]
    pub repeat: u32,
}

impl CellKey {
    /// The stable cross-run identity string (16 hex chars) used for diffing and
    /// as the server's per-run primary key. Includes `repeat` so trials are
    /// distinct.
    pub fn case_key(&self) -> CaseKey {
        let mut hasher = Sha256::new();
        hasher.update(self.provider_id.as_bytes());
        hasher.update([0]);
        hasher.update(self.prompt_id.as_deref().unwrap_or("").as_bytes());
        hasher.update([0]);
        hasher.update(self.test_id.as_bytes());
        hasher.update([0]);
        hasher.update(self.repeat.to_le_bytes());
        let digest = hasher.finalize();
        let hex: String = digest[..8].iter().map(|b| format!("{b:02x}")).collect();
        CaseKey::new(hex)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum CaseStatus {
    Pass,
    Fail,
    /// Infrastructure failure (provider error, cache-only miss). Never counted
    /// as a plain assertion failure.
    Error,
    /// Filtered out / not applicable to this cell.
    Skip,
}

impl CaseStatus {
    /// The wire string for this status (identical to its serde encoding).
    pub fn as_str(self) -> &'static str {
        match self {
            CaseStatus::Pass => "pass",
            CaseStatus::Fail => "fail",
            CaseStatus::Error => "error",
            CaseStatus::Skip => "skip",
        }
    }
}

impl std::str::FromStr for CaseStatus {
    type Err = String;

    /// Strict parse of the same strings [`CaseStatus::as_str`] produces.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pass" => Ok(CaseStatus::Pass),
            "fail" => Ok(CaseStatus::Fail),
            "error" => Ok(CaseStatus::Error),
            "skip" => Ok(CaseStatus::Skip),
            other => Err(format!(
                "invalid case status '{other}'; expected one of: pass, fail, error, skip"
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum AssertStatus {
    Pass,
    Fail,
    Error,
    /// Not evaluated because the case was already decided (short-circuit).
    Skipped,
}

/// The result of a single assertion.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[ts(optional_fields)]
pub struct AssertResult {
    pub kind: AssertName,
    pub status: AssertStatus,
    pub score: f64,
    pub weight: f64,
    #[serde(default)]
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
    /// The assertion's definition — its type-specific criteria as authored (the
    /// `contains` substring, the `llm-rubric` rubric text + threshold, …), plus
    /// a `negate: true` entry when the assertion is negated. `weight` is omitted
    /// (already a field above). Absent on pre-v2.1 stored blobs; presence-gated
    /// by the web UI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub criteria: Option<serde_json::Value>,
    #[serde(default)]
    pub cached: bool,
}

/// The result of one matrix cell.
///
/// The web UI receives this document verbatim from `GET /runs/{id}/cases/{key}`
/// (the stored blob, not a re-derived DTO), so every `skip_serializing_if`
/// here means "key absent on the wire". Field-level `#[ts(optional)]` is used
/// instead of struct-level `#[ts(optional_fields)]` because the struct-level
/// form only marks `Option` fields and would emit `tags` as required even
/// though it is skipped when empty; without it, ts-rs's serde-aware fallback
/// (`default` + `skip_serializing_if`) marks `tags` optional too.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
pub struct CaseResult {
    pub cell: CellKey,
    pub case_key: CaseKey,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// The rendered test variables that produced this cell (the substituted
    /// `vars` map, environment excluded — the same values fed to the provider
    /// request). Empty when the test had no vars, and absent on pre-v2.1 stored
    /// blobs; presence-gated by the web UI. Marked optional in TS via ts-rs's
    /// serde-aware fallback (`default` + `skip_serializing_if`), same as `tags`.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub vars: serde_json::Map<String, serde_json::Value>,
    pub status: CaseStatus,
    pub score: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub output: Option<Output>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub prompt: Option<RenderedPrompt>,
    /// The request the provider actually sent — the payload the model received,
    /// captured from the same code path that performs the call
    /// ([`crate::provider::Provider::request_preview`]).
    ///
    /// `prompt` above is what domarinn rendered; this is what crossed the wire,
    /// including the model id and every sampling parameter. The two differ in
    /// ways that matter when debugging: a `max_tokens` visible here next to a
    /// `length` stop reason explains a truncated case immediately.
    ///
    /// Absent when the provider declines to describe its request (the HTTP
    /// provider does, to avoid persisting `env`-templated credentials) and on
    /// stored blobs written before this field existed; presence-gated by the web
    /// UI. Never contains headers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "unknown")]
    pub request: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub stop_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "unknown")]
    pub raw: Option<serde_json::Value>,
    #[serde(default)]
    pub asserts: Vec<AssertResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub usage: Option<TokenUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub cost_usd: Option<f64>,
    pub latency_ms: u64,
    #[serde(default)]
    pub cached: bool,
    #[serde(default)]
    pub attempts: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, TS)]
#[ts(optional_fields)]
pub struct RunSummary {
    pub total: u64,
    pub passed: u64,
    pub failed: u64,
    pub errored: u64,
    pub skipped: u64,
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(default)]
    pub cache_hits: u64,
    #[serde(default)]
    pub cache_misses: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, TS)]
#[ts(optional_fields)]
pub struct GitMeta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(default)]
    pub dirty: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, TS)]
#[ts(optional_fields)]
pub struct CiMeta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_url: Option<String>,
}

/// Which filters produced this run (for reproducibility and the UI).
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, TS)]
pub struct FilterSpec {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filters: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub providers: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prompts: Vec<String>,
}

/// The full result of a run.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[ts(optional_fields)]
pub struct RunResult {
    pub schema_version: u32,
    pub run_id: RunId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suite: Option<String>,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub config_digest: String,
    pub config_snapshot: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<GitMeta>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ci: Option<CiMeta>,
    #[serde(default)]
    pub filters: FilterSpec,
    pub cases: Vec<CaseResult>,
    pub summary: RunSummary,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn case_key_is_deterministic_and_repeat_sensitive() {
        let base = CellKey {
            provider_id: "p".into(),
            prompt_id: Some("prompt".into()),
            test_id: "t".into(),
            repeat: 0,
        };
        let mut r1 = base.clone();
        r1.repeat = 1;
        assert_eq!(base.case_key(), base.case_key());
        assert_ne!(base.case_key(), r1.case_key());
        assert_eq!(base.case_key().as_str().len(), 16);
    }

    #[test]
    fn case_key_distinguishes_prompt_presence() {
        let with = CellKey {
            provider_id: "p".into(),
            prompt_id: Some("x".into()),
            test_id: "t".into(),
            repeat: 0,
        };
        let without = CellKey {
            prompt_id: None,
            ..with.clone()
        };
        assert_ne!(with.case_key(), without.case_key());
    }

    #[test]
    fn v1_case_result_deserializes_with_absent_v2_fields_and_re_serializes_byte_stable() {
        // A v1 result document has none of the added optional keys. It must
        // deserialize with each defaulting to its empty/`None` value, and —
        // because they carry `skip_serializing_if` — re-serialize without
        // emitting any of them, so a stored-then-reloaded v1 document is
        // byte-identical (the server's content-hash idempotency depends on
        // absent fields staying absent). This guards `prompt`/`stop_reason`/`raw`
        // and the later-added `vars` (case level) and `criteria` (assert level).
        let v1 = r#"{
            "cell": {"provider_id": "p", "test_id": "t"},
            "case_key": "0011223344556677",
            "status": "pass",
            "score": 1.0,
            "output": "hello",
            "asserts": [
                {"kind": "contains", "status": "pass", "score": 1.0,
                 "weight": 1.0, "reason": "ok", "cached": false}
            ],
            "latency_ms": 12
        }"#;
        let case: CaseResult = serde_json::from_str(v1).unwrap();
        assert!(case.prompt.is_none());
        assert!(case.stop_reason.is_none());
        assert!(case.raw.is_none());
        assert!(case.vars.is_empty());
        assert!(case.asserts[0].criteria.is_none());

        let reserialized = serde_json::to_string(&case).unwrap();
        assert!(!reserialized.contains("prompt"));
        assert!(!reserialized.contains("stop_reason"));
        assert!(!reserialized.contains("\"raw\""));
        assert!(!reserialized.contains("vars"));
        assert!(!reserialized.contains("criteria"));
    }

    #[test]
    fn case_status_as_str_matches_serde_and_round_trips_via_from_str() {
        for status in [
            CaseStatus::Pass,
            CaseStatus::Fail,
            CaseStatus::Error,
            CaseStatus::Skip,
        ] {
            let serde_str = serde_json::to_value(status).unwrap();
            assert_eq!(serde_str, serde_json::json!(status.as_str()));
            assert_eq!(status.as_str().parse::<CaseStatus>().unwrap(), status);
        }
        assert!("bogus".parse::<CaseStatus>().is_err());
    }
}
