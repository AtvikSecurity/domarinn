//! DTOs for `GET /runs`, `GET /runs/{id}`, `POST /runs`, and the lean
//! per-case assert record stored in the `cases.asserts` DB column.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use domarinn_core::asserts::AssertName;
use domarinn_core::ids::RunId;

/// A lean per-assert record: no reason/details/weight, just enough to render
/// a pass/fail chip. Serialized as a JSON array into the `cases.asserts` DB
/// column at ingest time and deserialized back out for `GET /runs/{id}/cases`.
/// `label` and `kind` are always equal today (both are the assert's
/// [`AssertName`]) but kept as two fields to match the wire shape the UI
/// already consumes.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct CaseAssertLean {
    pub label: AssertName,
    pub kind: AssertName,
    pub passed: bool,
    pub score: f64,
}

/// One row of `GET /runs` — everything needed to render a run in a list,
/// without the case-level detail.
#[derive(Debug, Clone, Serialize, TS)]
pub struct RunListItem {
    pub id: RunId,
    pub project: Option<String>,
    pub suite: Option<String>,
    /// RFC3339.
    pub created_at: String,
    pub git_branch: Option<String>,
    pub git_commit: Option<String>,
    pub git_dirty: Option<bool>,
    pub case_count: i64,
    pub pass_count: i64,
    pub fail_count: i64,
    pub error_count: i64,
    pub pass_rate: f64,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub cost_usd: Option<f64>,
    pub duration_ms: i64,
    /// Provider-call cache counters (migration-6 `runs` columns, promoted from
    /// `RunSummary`). `None` for legacy pre-backfill rows and for
    /// failed-backfill rows carrying the -1 sentinel, which the query maps to
    /// `None`. A run is "fully cached" when `cache_misses == 0 && cache_hits > 0`.
    pub cache_hits: Option<i64>,
    pub cache_misses: Option<i64>,
    /// How many of this run's cases came back empty (migration-15
    /// `runs.empty_count`, counted off the cases at ingest).
    ///
    /// Omitted rather than null or zero, the same carve-out from the
    /// null-not-omitted convention that [`super::cases::CaseListItem::empty_reason`]
    /// documents. Absent means "nothing to report" and covers all three ways
    /// there is nothing: the run had no empty cases, the column was never
    /// backfilled (NULL), or the blob would not decode (the `-1` sentinel).
    /// A reader must not turn absence into a rendered `0` — for the last two
    /// the true count is unknown. `RunSummary.empty_counts` is absent under
    /// the same rule, so "absent" means one thing across both.
    #[ts(optional)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub empty_count: Option<u64>,
    /// Who ran it, as recorded by the client (`RunOrigin.actor`). `None` for
    /// runs from clients that predate provenance, and for runs whose author
    /// suppressed it.
    pub actor: Option<String>,
    /// The machine it ran on. Same caveats as `actor`.
    pub host: Option<String>,
    /// Who *uploaded* it, from the authenticated identity. Distinct from
    /// `actor` on purpose: this one is verified and server-side, that one is
    /// client-supplied and covers local runs. A shared CI token shows up here
    /// as the token's label, which is why both are surfaced.
    pub uploaded_by: Option<String>,
    /// The CI system that ran it, if any. Its presence *is* the "was this CI?"
    /// flag — see `OriginFilter`.
    pub ci_provider: Option<String>,
    pub ci_run_url: Option<String>,
    /// The run's human label (`--note`, else the suite's `description`).
    pub note: Option<String>,
    /// The domarinn build that produced the run.
    pub domarinn_version: Option<String>,
    pub tags: Vec<String>,
}

/// `GET /runs` response.
#[derive(Debug, Clone, Serialize, TS)]
pub struct RunListResponse {
    pub runs: Vec<RunListItem>,
    pub next_cursor: Option<String>,
    /// How many runs `cached=exclude` suppressed across the whole filtered
    /// set (not just this page). `None` unless the query was `cached=exclude`
    /// with no cursor (i.e. the first page).
    pub cached_hidden: Option<i64>,
}

/// `GET /runs/{id}` response: run metadata plus the tags and the distinct
/// set of assert labels seen across its cases (used to populate filter UIs
/// without a second round trip).
#[derive(Debug, Clone, Serialize, TS)]
pub struct RunDetailResponse {
    pub id: RunId,
    pub project: Option<String>,
    pub suite: Option<String>,
    /// RFC3339.
    pub created_at: String,
    /// RFC3339.
    pub uploaded_at: String,
    pub schema_version: i64,
    pub git_branch: Option<String>,
    pub git_commit: Option<String>,
    pub git_dirty: Option<bool>,
    pub ci_provider: Option<String>,
    pub ci_run_url: Option<String>,
    pub case_count: i64,
    pub pass_count: i64,
    pub fail_count: i64,
    pub error_count: i64,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub cost_usd: Option<f64>,
    pub duration_ms: i64,
    pub content_hash: String,
    pub uploaded_by: Option<String>,
    /// The run's config digest (migration-3 `runs.config_digest` column).
    /// `None` for legacy rows with no digest and for failed-backfill rows that
    /// carry the empty-string sentinel, which the query maps to `None`.
    pub config_digest: Option<String>,
    /// Provider-call cache counters (see [`RunListItem::cache_hits`]).
    pub cache_hits: Option<i64>,
    pub cache_misses: Option<i64>,
    /// Provider-side prompt-cache token counters (migration 12). `None` for
    /// runs stored before the columns existed — which is not the same as zero,
    /// and readers must render it as unknown rather than "no cache activity".
    pub cache_read_tokens: Option<i64>,
    pub cache_write_tokens: Option<i64>,
    /// What the cached cases would have cost. Actual spend is `cost_usd` minus
    /// this.
    pub cache_savings_usd: Option<f64>,
    /// What grading cost, which is **not** part of `cost_usd`: that is what the
    /// systems under test cost, and it is what a `cost:` assertion budgets.
    pub grader_cost_usd: Option<f64>,
    /// Run provenance — see the same-named fields on [`RunListItem`].
    pub actor: Option<String>,
    pub host: Option<String>,
    pub note: Option<String>,
    pub domarinn_version: Option<String>,
    pub tags: Vec<String>,
    pub assert_labels: Vec<String>,
    /// This run's empty-output cases tallied by reason. Counts *every* empty
    /// output, not just refusals — `refusal` is one key among an open set.
    ///
    /// Grouped from the same `cases` rows [`RunListItem::empty_count`] is
    /// counted from, so the list count, this map, and the case grid always
    /// agree. The stored document's own `summary.empty_counts` is left to
    /// export consumers and is deliberately not the source here.
    ///
    /// Omitted, never `{}`, when the run reported none, matching how
    /// `RunSummary` itself serializes it and how [`RunListItem::empty_count`]
    /// behaves.
    #[ts(optional)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub empty_counts: Option<BTreeMap<String, u64>>,
}

/// `POST /runs` success response body (201 Created, or 200 OK when identical
/// content already existed). The 409 conflict body is a distinct, ad-hoc
/// shape (`{"error": ..., "id": ...}`) and is not this type.
#[derive(Debug, Clone, Serialize, TS)]
pub struct IngestResponse {
    pub id: RunId,
    pub url: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn case_assert_lean_matches_todays_wire_shape() {
        let dto = CaseAssertLean {
            label: AssertName::Contains,
            kind: AssertName::Contains,
            passed: true,
            score: 1.0,
        };
        assert_eq!(
            serde_json::to_value(&dto).unwrap(),
            json!({
                "label": "contains",
                "kind": "contains",
                "passed": true,
                "score": 1.0,
            })
        );
    }

    #[test]
    fn run_list_item_matches_todays_wire_shape() {
        let dto = RunListItem {
            id: RunId::new("r-1"),
            project: Some("proj".to_string()),
            suite: Some("suite".to_string()),
            created_at: "2026-01-01T00:00:00+00:00".to_string(),
            git_branch: Some("main".to_string()),
            git_commit: Some("abc123".to_string()),
            git_dirty: Some(false),
            case_count: 2,
            pass_count: 1,
            fail_count: 1,
            error_count: 0,
            pass_rate: 0.5,
            prompt_tokens: 10,
            completion_tokens: 20,
            cost_usd: Some(0.0025),
            duration_ms: 30000,
            cache_hits: Some(1),
            cache_misses: Some(1),
            empty_count: Some(2),
            actor: Some("alice".to_string()),
            host: Some("runner-07".to_string()),
            uploaded_by: Some("ci-token".to_string()),
            ci_provider: Some("github".to_string()),
            ci_run_url: Some("https://ci.example/run/1".to_string()),
            note: Some("nightly smoke".to_string()),
            domarinn_version: Some("0.2.0".to_string()),
            tags: vec!["nightly".to_string()],
        };
        assert_eq!(
            serde_json::to_value(&dto).unwrap(),
            json!({
                "id": "r-1",
                "project": "proj",
                "suite": "suite",
                "created_at": "2026-01-01T00:00:00+00:00",
                "git_branch": "main",
                "git_commit": "abc123",
                "git_dirty": false,
                "case_count": 2,
                "pass_count": 1,
                "fail_count": 1,
                "error_count": 0,
                "pass_rate": 0.5,
                "prompt_tokens": 10,
                "completion_tokens": 20,
                "cost_usd": 0.0025,
                "duration_ms": 30000,
                "cache_hits": 1,
                "cache_misses": 1,
                "empty_count": 2,
                "actor": "alice",
                "host": "runner-07",
                "uploaded_by": "ci-token",
                "ci_provider": "github",
                "ci_run_url": "https://ci.example/run/1",
                "note": "nightly smoke",
                "domarinn_version": "0.2.0",
                "tags": ["nightly"],
            })
        );
    }

    #[test]
    fn run_list_item_nulls_absent_optionals() {
        // A run with no project/suite/git/cost must serialize those as
        // explicit JSON null, not omit the keys (json! never omitted them).
        let dto = RunListItem {
            id: RunId::new("r-2"),
            project: None,
            suite: None,
            created_at: "2026-01-01T00:00:00+00:00".to_string(),
            git_branch: None,
            git_commit: None,
            git_dirty: None,
            case_count: 0,
            pass_count: 0,
            fail_count: 0,
            error_count: 0,
            pass_rate: 0.0,
            prompt_tokens: 0,
            completion_tokens: 0,
            cost_usd: None,
            duration_ms: 0,
            cache_hits: None,
            cache_misses: None,
            empty_count: None,
            actor: None,
            host: None,
            uploaded_by: None,
            ci_provider: None,
            ci_run_url: None,
            note: None,
            domarinn_version: None,
            tags: vec![],
        };
        let v = serde_json::to_value(&dto).unwrap();
        for key in [
            "project",
            "suite",
            "git_branch",
            "git_commit",
            "git_dirty",
            "cost_usd",
            "cache_hits",
            "cache_misses",
            "actor",
            "host",
            "uploaded_by",
            "ci_provider",
            "ci_run_url",
            "note",
            "domarinn_version",
        ] {
            assert!(v.get(key).is_some(), "missing key {key}");
            assert!(
                v[key].is_null(),
                "expected {key} to be null, got {:?}",
                v[key]
            );
        }
        // `empty_count` is the exception, for the same reason
        // `CaseListItem::empty_reason` is: it is `#[ts(optional)]`, so "nothing
        // to report" is the key being absent, never a null and never a `0`.
        assert!(
            v.get("empty_count").is_none(),
            "empty_count must be omitted, not null: {v}"
        );
    }

    #[test]
    fn run_detail_response_matches_todays_wire_shape() {
        let dto = RunDetailResponse {
            id: RunId::new("r-1"),
            project: Some("proj".to_string()),
            suite: Some("suite".to_string()),
            created_at: "2026-01-01T00:00:00+00:00".to_string(),
            uploaded_at: "2026-01-01T00:00:01+00:00".to_string(),
            schema_version: 1,
            git_branch: Some("main".to_string()),
            git_commit: Some("abc123".to_string()),
            git_dirty: Some(false),
            ci_provider: Some("ci".to_string()),
            ci_run_url: Some("https://ci.example/run/1".to_string()),
            case_count: 1,
            pass_count: 1,
            fail_count: 0,
            error_count: 0,
            prompt_tokens: 10,
            completion_tokens: 20,
            cost_usd: Some(0.0025),
            duration_ms: 30000,
            content_hash: "sha256:deadbeef".to_string(),
            uploaded_by: Some("alice".to_string()),
            config_digest: Some("sha256:cfg".to_string()),
            cache_hits: Some(1),
            cache_misses: Some(0),
            cache_read_tokens: Some(104_000),
            cache_write_tokens: Some(8_200),
            cache_savings_usd: Some(0.0011),
            grader_cost_usd: Some(0.0009),
            actor: Some("alice".to_string()),
            host: Some("runner-07".to_string()),
            note: Some("nightly smoke".to_string()),
            domarinn_version: Some("0.2.0".to_string()),
            tags: vec!["nightly".to_string()],
            assert_labels: vec!["contains".to_string(), "regex".to_string()],
            empty_counts: Some(BTreeMap::from([
                ("refusal".to_string(), 2),
                ("tool_use_only".to_string(), 1),
            ])),
        };
        assert_eq!(
            serde_json::to_value(&dto).unwrap(),
            json!({
                "id": "r-1",
                "project": "proj",
                "suite": "suite",
                "created_at": "2026-01-01T00:00:00+00:00",
                "uploaded_at": "2026-01-01T00:00:01+00:00",
                "schema_version": 1,
                "git_branch": "main",
                "git_commit": "abc123",
                "git_dirty": false,
                "ci_provider": "ci",
                "ci_run_url": "https://ci.example/run/1",
                "case_count": 1,
                "pass_count": 1,
                "fail_count": 0,
                "error_count": 0,
                "prompt_tokens": 10,
                "completion_tokens": 20,
                "cost_usd": 0.0025,
                "duration_ms": 30000,
                "content_hash": "sha256:deadbeef",
                "uploaded_by": "alice",
                "config_digest": "sha256:cfg",
                "cache_hits": 1,
                "cache_misses": 0,
                "cache_read_tokens": 104000,
                "cache_write_tokens": 8200,
                "cache_savings_usd": 0.0011,
                "grader_cost_usd": 0.0009,
                "actor": "alice",
                "host": "runner-07",
                "note": "nightly smoke",
                "domarinn_version": "0.2.0",
                "tags": ["nightly"],
                "assert_labels": ["contains", "regex"],
                "empty_counts": { "refusal": 2, "tool_use_only": 1 },
            })
        );
    }

    #[test]
    fn run_detail_response_nulls_absent_config_digest() {
        // A run with no digest must serialize `config_digest` as explicit JSON
        // null, not omit the key (DTO null-not-omitted convention).
        let dto = RunDetailResponse {
            id: RunId::new("r-3"),
            project: None,
            suite: None,
            created_at: "2026-01-01T00:00:00+00:00".to_string(),
            uploaded_at: "2026-01-01T00:00:01+00:00".to_string(),
            schema_version: 1,
            git_branch: None,
            git_commit: None,
            git_dirty: None,
            ci_provider: None,
            ci_run_url: None,
            case_count: 0,
            pass_count: 0,
            fail_count: 0,
            error_count: 0,
            prompt_tokens: 0,
            completion_tokens: 0,
            cost_usd: None,
            duration_ms: 0,
            content_hash: "sha256:deadbeef".to_string(),
            uploaded_by: None,
            config_digest: None,
            cache_hits: None,
            cache_misses: None,
            cache_read_tokens: None,
            cache_write_tokens: None,
            cache_savings_usd: None,
            grader_cost_usd: None,
            actor: None,
            host: None,
            note: None,
            domarinn_version: None,
            tags: vec![],
            assert_labels: vec![],
            empty_counts: None,
        };
        let v = serde_json::to_value(&dto).unwrap();
        assert!(v.get("config_digest").is_some());
        assert!(v["config_digest"].is_null());
        assert!(v["cache_hits"].is_null());
        assert!(v["cache_misses"].is_null());
        // A run with no empty cases carries no map at all, not an empty one —
        // matching `RunSummary`, which omits the same field.
        assert!(
            v.get("empty_counts").is_none(),
            "empty_counts must be omitted when there is nothing to tally: {v}"
        );
    }

    #[test]
    fn ingest_response_matches_todays_wire_shape() {
        let dto = IngestResponse {
            id: RunId::new("run-1"),
            url: "http://localhost/runs/run-1".to_string(),
        };
        assert_eq!(
            serde_json::to_value(&dto).unwrap(),
            json!({
                "id": "run-1",
                "url": "http://localhost/runs/run-1",
            })
        );
    }
}
