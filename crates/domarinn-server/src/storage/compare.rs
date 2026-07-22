//! Run/run comparison: load both runs' cases, join on `case_key`, classify
//! transitions. `output_changed` compares the stored `output_hash` only (no blob
//! loads).
//!
//! The per-case classification (see [`classify`]) is deliberately *not*
//! [`domarinn_core::diff::Delta`]: this endpoint special-cases a pass/pass
//! pair as `still_passing`, where core's `diff_runs` folds every same-status
//! pair (including e.g. skip/skip) into a single `unchanged`. See
//! [`crate::dto::compare::CompareDelta`]'s doc comment for the full story.

use std::collections::BTreeMap;
use std::str::FromStr;

use rusqlite::{params, Connection, OptionalExtension};

use domarinn_core::ids::{CaseKey, RunId};
use domarinn_core::result::CaseStatus;
use domarinn_core::stats::{mcnemar, wilson, Z_95};

use super::{empty_to_none, from_microusd, parse_stored_asserts, Storage};
use crate::dto::compare::{
    AssertFlip, CompareCaseRow, CompareConfig, CompareDelta, CompareResponse, CompareStats,
    CompareSummary, CompareTotals, RunTotals,
};
use crate::dto::runs::CaseAssertLean;

impl Storage {
    pub async fn compare_runs(
        &self,
        base: RunId,
        head: RunId,
    ) -> anyhow::Result<Option<CompareResponse>> {
        self.runs
            .read(move |conn| compare_runs(conn, &base, &head))
            .await
    }
}

struct CmpCase {
    name: Option<String>,
    status: CaseStatus,
    output_hash: Option<String>,
    score: Option<f64>,
    asserts: Vec<CaseAssertLean>,
}

/// Per-run aggregate columns from the `runs` table (column-only; no blob load).
struct RunAgg {
    prompt_tokens: i64,
    completion_tokens: i64,
    cost_microusd: Option<i64>,
    duration_ms: Option<i64>,
    case_count: i64,
    pass_count: i64,
    config_digest: Option<String>,
}

fn load_compare_cases(
    conn: &Connection,
    run_id: &str,
) -> anyhow::Result<BTreeMap<String, CmpCase>> {
    let mut stmt = conn.prepare(
        "SELECT case_key, name, status, output_hash, score, asserts
         FROM cases WHERE run_id = ?1",
    )?;
    let rows = stmt.query_map(params![run_id], |row| {
        let case_key: String = row.get(0)?;
        let status_raw: String = row.get(2)?;
        let status = CaseStatus::from_str(&status_raw).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, e.into())
        })?;
        // Graceful degrade, matching the case-list read path (see
        // `parse_stored_asserts`): a corrupt/tampered row contributes no flips.
        let asserts_str: Option<String> = row.get(5)?;
        let asserts = parse_stored_asserts(asserts_str, run_id, &case_key);
        Ok((
            case_key,
            CmpCase {
                name: row.get(1)?,
                status,
                output_hash: row.get(3)?,
                score: row.get(4)?,
                asserts,
            },
        ))
    })?;
    let mut map = BTreeMap::new();
    for row in rows {
        let (k, v) = row?;
        map.insert(k, v);
    }
    Ok(map)
}

/// Load a run's aggregate columns. `None` means the run does not exist (drives
/// the endpoint's 404).
fn load_run_agg(conn: &Connection, run_id: &str) -> anyhow::Result<Option<RunAgg>> {
    let agg = conn
        .query_row(
            "SELECT prompt_tokens, completion_tokens, cost_microusd, duration_ms,
                    case_count, pass_count, config_digest
             FROM runs WHERE id = ?1",
            params![run_id],
            |row| {
                Ok(RunAgg {
                    prompt_tokens: row.get(0)?,
                    completion_tokens: row.get(1)?,
                    cost_microusd: row.get(2)?,
                    duration_ms: row.get(3)?,
                    case_count: row.get(4)?,
                    pass_count: row.get(5)?,
                    config_digest: row.get(6)?,
                })
            },
        )
        .optional()?;
    Ok(agg)
}

fn to_run_totals(agg: &RunAgg) -> RunTotals {
    RunTotals {
        prompt_tokens: agg.prompt_tokens,
        completion_tokens: agg.completion_tokens,
        cost_usd: from_microusd(agg.cost_microusd),
        duration_ms: agg.duration_ms,
        case_count: agg.case_count,
        pass_count: agg.pass_count,
    }
}

/// Pair the two runs' lean asserts for one case and emit the pass/fail flips.
///
/// Lean records carry no stable per-assert id, so pairing is heuristic:
/// positionally when both runs report the exact same kind-sequence, otherwise
/// by kind for kinds that occur exactly once on each side. Anything not paired
/// that way is dropped (no flip emitted).
fn compute_assert_flips(base: &[CaseAssertLean], head: &[CaseAssertLean]) -> Vec<AssertFlip> {
    let base_kinds: Vec<_> = base.iter().map(|a| a.kind).collect();
    let head_kinds: Vec<_> = head.iter().map(|a| a.kind).collect();
    let mut flips = Vec::new();

    if base_kinds == head_kinds {
        // Identical kind-sequences: pair positionally.
        for (b, h) in base.iter().zip(head.iter()) {
            if b.passed != h.passed {
                flips.push(mk_flip(b, h));
            }
        }
    } else {
        // Fall back to pairing by kind, but only for kinds that occur exactly
        // once on each side (otherwise the pairing is ambiguous). Iterate in
        // base order for a deterministic result.
        for b in base {
            let base_count = base.iter().filter(|a| a.kind == b.kind).count();
            let head_count = head.iter().filter(|a| a.kind == b.kind).count();
            if base_count == 1 && head_count == 1 {
                if let Some(h) = head.iter().find(|a| a.kind == b.kind) {
                    if b.passed != h.passed {
                        flips.push(mk_flip(b, h));
                    }
                }
            }
        }
    }
    flips
}

fn mk_flip(b: &CaseAssertLean, h: &CaseAssertLean) -> AssertFlip {
    AssertFlip {
        kind: b.kind,
        base_passed: b.passed,
        head_passed: h.passed,
        base_score: b.score,
        head_score: h.score,
    }
}

fn is_failing(status: CaseStatus) -> bool {
    matches!(status, CaseStatus::Fail | CaseStatus::Error)
}

/// Classify one case's transition from `b` (base, if present) to `h` (head,
/// if present). Mirrors today's endpoint behavior exactly (see the module
/// doc): pass/pass is its own `StillPassing` variant, and anything else with
/// matching presence-but-non-failing/non-passing status (in practice, a
/// `skip` on either side) falls back to `Unchanged`.
fn classify(b: Option<&CmpCase>, h: Option<&CmpCase>) -> CompareDelta {
    match (b, h) {
        (Some(b), Some(h)) => {
            if b.status == CaseStatus::Pass && is_failing(h.status) {
                CompareDelta::NewlyFailing
            } else if is_failing(b.status) && h.status == CaseStatus::Pass {
                CompareDelta::NewlyPassing
            } else if is_failing(b.status) && is_failing(h.status) {
                CompareDelta::StillFailing
            } else if b.status == CaseStatus::Pass && h.status == CaseStatus::Pass {
                CompareDelta::StillPassing
            } else {
                CompareDelta::Unchanged
            }
        }
        (None, Some(_)) => CompareDelta::Added,
        (Some(_), None) => CompareDelta::Removed,
        (None, None) => unreachable!(),
    }
}

fn compare_runs(
    conn: &Connection,
    base: &RunId,
    head: &RunId,
) -> anyhow::Result<Option<CompareResponse>> {
    // A missing aggregate row means the run does not exist → 404.
    let Some(base_agg) = load_run_agg(conn, base.as_str())? else {
        return Ok(None);
    };
    let Some(head_agg) = load_run_agg(conn, head.as_str())? else {
        return Ok(None);
    };

    let base_cases = load_compare_cases(conn, base.as_str())?;
    let head_cases = load_compare_cases(conn, head.as_str())?;

    let mut keys: Vec<String> = base_cases.keys().cloned().collect();
    for k in head_cases.keys() {
        if !base_cases.contains_key(k) {
            keys.push(k.clone());
        }
    }
    keys.sort();

    let mut summary = CompareSummary {
        newly_failing: 0,
        newly_passing: 0,
        still_failing: 0,
        output_changed: 0,
        added: 0,
        removed: 0,
    };
    let mut cases = Vec::new();

    for key in keys {
        let b = base_cases.get(&key);
        let h = head_cases.get(&key);
        let delta = classify(b, h);
        let out_changed = match (b, h) {
            (Some(b), Some(h)) => b.output_hash != h.output_hash,
            _ => false,
        };

        match delta {
            CompareDelta::NewlyFailing => summary.newly_failing += 1,
            CompareDelta::NewlyPassing => summary.newly_passing += 1,
            CompareDelta::StillFailing => summary.still_failing += 1,
            CompareDelta::Added => summary.added += 1,
            CompareDelta::Removed => summary.removed += 1,
            CompareDelta::StillPassing | CompareDelta::Unchanged => {}
        }
        if out_changed {
            summary.output_changed += 1;
        }

        let name = h
            .and_then(|c| c.name.clone())
            .or_else(|| b.and_then(|c| c.name.clone()));
        let base_score = b.and_then(|c| c.score);
        let head_score = h.and_then(|c| c.score);
        let score_delta = match (base_score, head_score) {
            (Some(bs), Some(hs)) => Some(hs - bs),
            _ => None,
        };
        let assert_flips = match (b, h) {
            (Some(b), Some(h)) => compute_assert_flips(&b.asserts, &h.asserts),
            _ => Vec::new(),
        };
        cases.push(CompareCaseRow {
            case_key: CaseKey::new(key),
            name,
            base_status: b.map(|c| c.status),
            head_status: h.map(|c| c.status),
            delta,
            output_changed: out_changed,
            base_score,
            head_score,
            score_delta,
            assert_flips,
        });
    }

    // McNemar is fed the discordant-pair counts the transition tally already
    // computed: regressions (newly failing) and fixes (newly passing).
    let stats = CompareStats {
        mcnemar: mcnemar(summary.newly_failing, summary.newly_passing).into(),
        base_pass_rate: wilson(
            base_agg.pass_count.max(0) as u64,
            base_agg.case_count.max(0) as u64,
            Z_95,
        )
        .into(),
        head_pass_rate: wilson(
            head_agg.pass_count.max(0) as u64,
            head_agg.case_count.max(0) as u64,
            Z_95,
        )
        .into(),
    };

    let totals = CompareTotals {
        base: to_run_totals(&base_agg),
        head: to_run_totals(&head_agg),
    };

    // Map the empty-string backfill sentinel (and NULL) to `None`; `changed`
    // is only meaningful when *both* digests are known.
    let base_digest = empty_to_none(base_agg.config_digest);
    let head_digest = empty_to_none(head_agg.config_digest);
    let changed = match (&base_digest, &head_digest) {
        (Some(b), Some(h)) => Some(b != h),
        _ => None,
    };
    let config = CompareConfig {
        base_digest,
        head_digest,
        changed,
    };

    Ok(Some(CompareResponse {
        base: base.clone(),
        head: head.clone(),
        summary,
        cases,
        stats,
        totals,
        config,
    }))
}
