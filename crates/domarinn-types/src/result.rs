//! The versioned [`RunResult`] document — the seam between engine, CLI, and
//! server. Bump [`RESULT_SCHEMA_VERSION`] on any breaking change; golden
//! snapshots guard it.

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use ts_rs::TS;

use crate::assert_name::AssertName;
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

impl AssertStatus {
    /// The status of a graded assertion from its boolean outcome.
    ///
    /// A constructor for the wire value, so it lives with the type rather than
    /// with whichever engine module happens to call it.
    pub fn from_pass(pass: bool) -> AssertStatus {
        if pass {
            AssertStatus::Pass
        } else {
            AssertStatus::Fail
        }
    }
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
    /// Time the provider itself took, excluding retry backoff and (on a cache
    /// hit) replayed from the entry rather than measured against the cache read.
    ///
    /// This is what `latency` assertions grade. Keeping backoff out of it is
    /// not cosmetic: with retries enabled, a single 429 would otherwise fail
    /// `{type: latency, max: 2000}` on a model that answered in 30 ms.
    pub latency_ms: u64,
    /// Total wall time for the cell, *including* retry backoff and cache I/O.
    /// Absent on blobs written before this field existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub wall_ms: Option<u64>,
    #[serde(default)]
    pub cached: bool,
    #[serde(default)]
    pub attempts: u32,
    /// Identity of what this case asked the model — rendered prompt, rendered
    /// vars, params, cache salt. Deliberately excludes the provider (that is
    /// `provider_digest`) and `repeat`. See [`crate::digests::prompt_digest`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub prompt_digest: Option<String>,
    /// Identity of the model and its settings, from the provider fingerprint.
    ///
    /// **Not backfillable**: the fingerprint appears nowhere in a stored run
    /// document (`request` is a preview envelope and is absent under
    /// `--no-raw`), so change classification only works between two runs that
    /// both post-date this field. Readers must treat `None` as unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub provider_digest: Option<String>,
    /// Identity of this case's grading definition — the authored criteria,
    /// weights and threshold, never the outcome.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub assert_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub error: Option<String>,
    /// Structured detail for whatever `error` describes — the machine-readable
    /// half of a message that is otherwise prose.
    ///
    /// Provider-authored for a provider failure. It exists because `error` was
    /// the only field that survived a failed call, so anything structured had
    /// to be formatted into a sentence and parsed back out by whoever read it.
    ///
    /// Size-capped like `raw`, but deliberately **not** gated behind
    /// `--no-raw`: that flag exists to shrink documents by dropping bulky
    /// happy-path provenance, and an errored case has no `output` and no `raw`,
    /// so this is the only thing it carries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    #[ts(type = "unknown")]
    pub error_details: Option<serde_json::Value>,
    /// What kind of failure `error` describes — see [`crate::error_class`].
    ///
    /// Strictly additive: `error` stays the verbatim prose it always was, and
    /// neither field is ever derived from the other at read time.
    /// `error_class.is_some()` implies `error.is_some()`; the converse does not
    /// hold, because blobs written before this field have prose and no class.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub error_class: Option<crate::error_class::ErrorClass>,
    /// The model's reasoning/thinking text, when it exposed any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub reasoning: Option<String>,
    /// Why `output` has nothing gradeable in it. Set whenever the output is
    /// empty, including when reasoning was substituted for it — renderers must
    /// key on this being present, never on the output looking empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub empty_reason: Option<crate::empty::EmptyReason>,
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
    /// Cases that needed more than one attempt. Distinct from `errored`: a
    /// retried case usually *succeeded*, and without this the only trace of the
    /// cost is a longer wall-clock.
    #[serde(default)]
    pub retried_cases: u64,
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

/// Who and what produced a run.
///
/// Collected by the engine (see [`crate::provenance`]) so a plain `domarinn run`
/// carries it too. Before this existed, git and CI metadata were attached only
/// on the upload path, so every `result.json` on disk had `git: null` and there
/// was no notion of an actor at all.
///
/// [`RunResult::git`] and [`RunResult::ci`] deliberately stay as their own
/// fields rather than moving in here: they already have server columns and a
/// share-time backfill path, so folding them in would be a breaking rename for
/// no gain.
///
/// One optional struct rather than five optional fields, also deliberately. The
/// byte-stable-reserialization contract is about keys staying *absent* on
/// documents that never had them; nesting costs exactly one
/// `skip_serializing_if` at the [`RunResult`] level and leaves every field
/// inside it unconstrained for legacy blobs, which carry no `origin` key at all.
/// It also gives the privacy opt-out a single lever: `origin: None`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, TS)]
#[ts(optional_fields)]
pub struct RunOrigin {
    /// Who this run is attributable to: the CI actor when running in CI
    /// (`GITHUB_ACTOR` and friends — of a CI run people ask "who pushed the
    /// change", not "which service account ran the container"), otherwise the OS
    /// username. `None` when suppressed or undeterminable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    /// Hostname of the machine that ran it. `None` when suppressed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// The domarinn build that produced this document ([`crate::VERSION`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// A free-form human label (`domarinn run --note "…"`), falling back to the
    /// suite's `description`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Set when the identity fields were suppressed by policy. Distinguishes "the
    /// operator opted out" from "an older client wrote this document" — without
    /// it a reader cannot tell the two apart, since both look like absence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redacted: Option<bool>,
}

/// Component digests of the suite that produced a run — see [`crate::digests`].
///
/// [`RunResult::config_digest`] is one hash over the whole suite: it answers
/// "did anything change" and nothing else. These answer *what*, so a comparison
/// can say "the prompts changed" instead of "config changed", and the runs list
/// can say it from the row without fetching a config snapshot per run.
///
/// All optional: a run written before this existed has none, and `None` must
/// read as "unknown", never as "unchanged".
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, TS)]
#[ts(optional_fields)]
pub struct ConfigDigests {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompts: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub providers: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tests: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asserts: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grader: Option<String>,
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
    /// Who and where this run came from. See [`RunOrigin`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<RunOrigin>,
    /// Per-component digests of the suite. See [`ConfigDigests`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digests: Option<ConfigDigests>,
    /// Where this run was published, recorded by `--share` from the URL the
    /// results server returned. Absent when the run was never shared.
    ///
    /// Stored rather than re-derived so a later reader — `ci-summary` building a
    /// PR comment, most of all — can link to the run without re-uploading it or
    /// parsing it back out of an earlier command's stdout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share_url: Option<String>,
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
        // The later-added digest and classification fields. `prompt_digest`
        // would be caught by the `prompt` assertion above only by accident;
        // the other three had no guard at all, so dropping a
        // `skip_serializing_if` from any of them used to pass silently — and
        // every historical run's content hash would shift on re-ingest.
        assert!(!reserialized.contains("prompt_digest"));
        assert!(!reserialized.contains("provider_digest"));
        assert!(!reserialized.contains("assert_digest"));
        assert!(!reserialized.contains("error_class"));
    }

    /// The run-level counterpart of the guard above.
    ///
    /// The server's ingest idempotency key is `sha256(canonical_json(run))` over
    /// the *whole* document, so a stored run that never carried `origin` (or
    /// `git`, `ci`, `share_url`) must not grow the key on a load/store round
    /// trip — that would change its content hash and turn an idempotent
    /// re-upload into a 409 Conflict.
    #[test]
    fn a_run_without_optional_provenance_does_not_grow_it_on_re_serialization() {
        let stored = r#"{
            "schema_version": 2,
            "run_id": "01ARZ3NDEKTSV4RRFFQ69G5FAV",
            "started_at": "2026-01-01T00:00:00Z",
            "finished_at": "2026-01-01T00:00:01Z",
            "config_digest": "blake3:abc",
            "config_snapshot": {},
            "cases": [],
            "summary": {"total": 0, "passed": 0, "failed": 0, "errored": 0, "skipped": 0}
        }"#;
        let run: RunResult = serde_json::from_str(stored).unwrap();
        assert!(run.origin.is_none());
        assert!(run.digests.is_none());
        assert!(run.git.is_none());
        assert!(run.ci.is_none());
        assert!(run.share_url.is_none());

        let reserialized = serde_json::to_string(&run).unwrap();
        assert!(!reserialized.contains("origin"));
        assert!(!reserialized.contains("\"git\""));
        assert!(!reserialized.contains("\"ci\""));
        assert!(!reserialized.contains("share_url"));
        assert!(!reserialized.contains("digests"));
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
