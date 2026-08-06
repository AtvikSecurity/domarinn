//! DTOs for `GET /runs/{id}/matrix`.
//!
//! A per-run aggregate that powers the UI's prompt × provider matrix: rows are
//! tests, columns are `(provider, prompt)` pairs, and each cell collapses every
//! repeat of that test × column into small flakiness-aware aggregates. Built
//! server-side because client-side pivoting is not viable — `/cases` is
//! paginated and carries heavy previews, whereas the matrix needs every cell
//! but only tiny aggregates.
//!
//! `columns` are always the complete set for the run (they are small); only
//! `rows` paginate, so a row's `cells` is aligned 1:1 with `columns` and a
//! `None` marks a test that never ran on that column.

use serde::Serialize;
use ts_rs::TS;

use domarinn_core::ids::{CaseKey, RunId};

/// `GET /runs/{id}/matrix` response.
///
/// `columns` are in first-seen (`idx`) order and complete; `rows` are in
/// first-seen order and paginate. Pass `next_cursor` back as `cursor` to page.
#[derive(Debug, Clone, Serialize, TS)]
pub struct MatrixResponse {
    pub run_id: RunId,
    pub columns: Vec<MatrixColumn>,
    pub rows: Vec<MatrixRow>,
    /// Per-provider cost for the whole run, attributed to the provider that
    /// **answered** — not the one the cell was configured with. Computed over
    /// every case in the run, so it does not move as `rows` paginate.
    ///
    /// A cell whose configured provider refused or failed can be answered by a
    /// fallback, and the fallback is what spent the tokens. Two consequences
    /// worth reading before using this: an entry can name a provider that
    /// forms no [`MatrixColumn`] at all (it only ever answered for someone
    /// else), and rows stored before this attribution existed carry no answerer
    /// and so degrade to being billed to their configured provider.
    ///
    /// In first-seen order, matching `columns`.
    pub provider_costs: Vec<ProviderCost>,
    pub next_cursor: Option<String>,
}

/// What one provider spent across a whole run, keyed by who *answered*.
///
/// Deliberately not keyed the way [`MatrixColumn`] is: columns stay keyed on
/// the configured provider so a cell's identity is stable across runs, while
/// cost follows the provider that actually made the call.
#[derive(Debug, Clone, Serialize, TS)]
pub struct ProviderCost {
    pub provider_id: String,
    /// How many of the run's cases this provider answered.
    pub cases: i64,
    /// Summed cost across those cases; `None` when every one of them recorded
    /// a NULL cost.
    pub cost_usd: Option<f64>,
}

/// One matrix column: a distinct `(provider, prompt)` pair. `prompt_id` is
/// `None` for cells with no prompt dimension.
#[derive(Debug, Clone, Serialize, TS)]
pub struct MatrixColumn {
    pub provider_id: String,
    pub prompt_id: Option<String>,
}

/// One matrix row: a test and its cells. `cells` is aligned 1:1 with
/// [`MatrixResponse::columns`]; a `None` entry means this test never ran on
/// that column.
#[derive(Debug, Clone, Serialize, TS)]
pub struct MatrixRow {
    pub test_id: String,
    /// First non-null case name seen for this test, if any.
    pub name: Option<String>,
    pub cells: Vec<Option<MatrixCell>>,
}

/// A single matrix cell: every repeat of one test × column collapsed into
/// status counts plus flakiness signals.
#[derive(Debug, Clone, Serialize, TS)]
pub struct MatrixCell {
    pub total: i64,
    pub passed: i64,
    pub failed: i64,
    pub errored: i64,
    pub skipped: i64,
    /// Mean of the non-null scores; `None` when no repeat carried a score.
    pub score_mean: Option<f64>,
    /// `passed / total`.
    pub pass_fraction: f64,
    /// Count of distinct non-null `output_hash` values across the repeats — a
    /// flakiness signal (`> 1` means the output was not stable).
    pub distinct_outputs: i64,
    /// Mean latency across repeats that recorded one; `None` if none did.
    pub latency_ms_mean: Option<f64>,
    /// Summed cost across repeats; `None` when every repeat's cost was NULL.
    pub cost_usd: Option<f64>,
    /// How many of this cell's repeats were answered by a provider other than
    /// the column's configured one (a fallback stood in). `0` for a run stored
    /// before the attribution existed — honestly so: fallback did not exist
    /// then, so no repeat in it had one.
    pub fallback_answered: i64,
    /// The cell's case keys, ordered by `repeat_idx` (ties broken by `idx`).
    pub case_keys: Vec<CaseKey>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn matrix_response_matches_todays_wire_shape() {
        let dto = MatrixResponse {
            run_id: RunId::new("r-1"),
            columns: vec![
                MatrixColumn {
                    provider_id: "openai".to_string(),
                    prompt_id: Some("p-a".to_string()),
                },
                MatrixColumn {
                    provider_id: "anthropic".to_string(),
                    prompt_id: None,
                },
            ],
            rows: vec![MatrixRow {
                test_id: "t1".to_string(),
                name: Some("openai::t1".to_string()),
                cells: vec![
                    Some(MatrixCell {
                        total: 2,
                        passed: 1,
                        failed: 1,
                        errored: 0,
                        skipped: 0,
                        score_mean: Some(0.5),
                        pass_fraction: 0.5,
                        distinct_outputs: 2,
                        latency_ms_mean: Some(42.0),
                        cost_usd: Some(0.005),
                        fallback_answered: 1,
                        case_keys: vec![CaseKey::new("aaaa"), CaseKey::new("bbbb")],
                    }),
                    None,
                ],
            }],
            provider_costs: vec![
                ProviderCost {
                    provider_id: "openai".to_string(),
                    cases: 1,
                    cost_usd: Some(0.001),
                },
                // An answerer that formed no column of its own: it only ever
                // stood in for a configured provider that refused.
                ProviderCost {
                    provider_id: "reserve".to_string(),
                    cases: 1,
                    cost_usd: None,
                },
            ],
            next_cursor: Some("0".to_string()),
        };
        assert_eq!(
            serde_json::to_value(&dto).unwrap(),
            json!({
                "run_id": "r-1",
                "columns": [
                    { "provider_id": "openai", "prompt_id": "p-a" },
                    { "provider_id": "anthropic", "prompt_id": null },
                ],
                "rows": [
                    {
                        "test_id": "t1",
                        "name": "openai::t1",
                        "cells": [
                            {
                                "total": 2,
                                "passed": 1,
                                "failed": 1,
                                "errored": 0,
                                "skipped": 0,
                                "score_mean": 0.5,
                                "pass_fraction": 0.5,
                                "distinct_outputs": 2,
                                "latency_ms_mean": 42.0,
                                "cost_usd": 0.005,
                                "fallback_answered": 1,
                                "case_keys": ["aaaa", "bbbb"],
                            },
                            null,
                        ],
                    }
                ],
                "provider_costs": [
                    { "provider_id": "openai", "cases": 1, "cost_usd": 0.001 },
                    { "provider_id": "reserve", "cases": 1, "cost_usd": null },
                ],
                "next_cursor": "0",
            })
        );
    }

    #[test]
    fn matrix_optionals_serialize_as_null_not_omitted() {
        // A run with no cell columns yields empty `columns`/`rows`; and a cell's
        // optional aggregates serialize as explicit `null`, never omitted, so
        // the UI always sees the keys.
        let empty = MatrixResponse {
            run_id: RunId::new("r-2"),
            columns: vec![],
            rows: vec![],
            provider_costs: vec![],
            next_cursor: None,
        };
        let v = serde_json::to_value(&empty).unwrap();
        assert_eq!(v["columns"], json!([]));
        assert_eq!(v["rows"], json!([]));
        assert_eq!(v["provider_costs"], json!([]));
        assert!(v.get("next_cursor").is_some());
        assert!(v["next_cursor"].is_null());

        let cell = MatrixCell {
            total: 1,
            passed: 0,
            failed: 0,
            errored: 0,
            skipped: 1,
            score_mean: None,
            pass_fraction: 0.0,
            distinct_outputs: 0,
            latency_ms_mean: None,
            cost_usd: None,
            fallback_answered: 0,
            case_keys: vec![],
        };
        let cv = serde_json::to_value(&cell).unwrap();
        assert_eq!(cv["case_keys"], json!([]));
        assert_eq!(cv["fallback_answered"], json!(0));
        for key in ["score_mean", "latency_ms_mean", "cost_usd"] {
            assert!(cv.get(key).is_some(), "missing key {key}");
            assert!(cv[key].is_null(), "expected {key} null, got {:?}", cv[key]);
        }
    }
}
