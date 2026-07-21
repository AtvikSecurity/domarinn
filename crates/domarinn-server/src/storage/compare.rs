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

use rusqlite::{params, Connection};

use domarinn_core::ids::{CaseKey, RunId};
use domarinn_core::result::CaseStatus;

use super::Storage;
use crate::dto::compare::{CompareCaseRow, CompareDelta, CompareResponse, CompareSummary};

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
}

fn load_compare_cases(
    conn: &Connection,
    run_id: &str,
) -> anyhow::Result<BTreeMap<String, CmpCase>> {
    let mut stmt =
        conn.prepare("SELECT case_key, name, status, output_hash FROM cases WHERE run_id = ?1")?;
    let rows = stmt.query_map(params![run_id], |row| {
        let status_raw: String = row.get(2)?;
        let status = CaseStatus::from_str(&status_raw).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, e.into())
        })?;
        Ok((
            row.get::<_, String>(0)?,
            CmpCase {
                name: row.get(1)?,
                status,
                output_hash: row.get(3)?,
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
    let base_exists = conn
        .query_row(
            "SELECT 1 FROM runs WHERE id = ?1",
            params![base.as_str()],
            |_| Ok(()),
        )
        .is_ok();
    let head_exists = conn
        .query_row(
            "SELECT 1 FROM runs WHERE id = ?1",
            params![head.as_str()],
            |_| Ok(()),
        )
        .is_ok();
    if !base_exists || !head_exists {
        return Ok(None);
    }

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
        cases.push(CompareCaseRow {
            case_key: CaseKey::new(key),
            name,
            base_status: b.map(|c| c.status),
            head_status: h.map(|c| c.status),
            delta,
            output_changed: out_changed,
        });
    }

    Ok(Some(CompareResponse {
        base: base.clone(),
        head: head.clone(),
        summary,
        cases,
    }))
}
