//! The versioned [`RunResult`] document — the seam between engine, CLI, and
//! server. Bump [`RESULT_SCHEMA_VERSION`] on any breaking change; golden
//! snapshots guard it.

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use ts_rs::TS;

use crate::asserts::AssertName;
use crate::types::{Output, TokenUsage};

pub const RESULT_SCHEMA_VERSION: u32 = 1;

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
    pub fn case_key(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.provider_id.as_bytes());
        hasher.update([0]);
        hasher.update(self.prompt_id.as_deref().unwrap_or("").as_bytes());
        hasher.update([0]);
        hasher.update(self.test_id.as_bytes());
        hasher.update([0]);
        hasher.update(self.repeat.to_le_bytes());
        let digest = hasher.finalize();
        digest[..8].iter().map(|b| format!("{b:02x}")).collect()
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
    #[serde(default)]
    pub cached: bool,
}

/// The result of one matrix cell.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[ts(optional_fields)]
pub struct CaseResult {
    pub cell: CellKey,
    pub case_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    pub status: CaseStatus,
    pub score: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<Output>,
    #[serde(default)]
    pub asserts: Vec<AssertResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<TokenUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    pub latency_ms: u64,
    #[serde(default)]
    pub cached: bool,
    #[serde(default)]
    pub attempts: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
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
    pub run_id: String,
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
        assert_eq!(base.case_key().len(), 16);
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
