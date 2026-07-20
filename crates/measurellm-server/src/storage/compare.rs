//! Run/run comparison: load both runs' cases, join on `case_key`, classify
//! transitions. `output_changed` compares the stored `output_hash` only (no blob
//! loads).

use std::collections::BTreeMap;
use std::str::FromStr;

use rusqlite::{params, Connection};

use measurellm_core::result::CaseStatus;

use super::Storage;

impl Storage {
    pub async fn compare_runs(
        &self,
        base: String,
        head: String,
    ) -> anyhow::Result<Option<serde_json::Value>> {
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

fn compare_runs(
    conn: &Connection,
    base: &str,
    head: &str,
) -> anyhow::Result<Option<serde_json::Value>> {
    let base_exists = conn
        .query_row(
            "SELECT 1 FROM runs WHERE id = ?1",
            params![base],
            |_| Ok(()),
        )
        .is_ok();
    let head_exists = conn
        .query_row(
            "SELECT 1 FROM runs WHERE id = ?1",
            params![head],
            |_| Ok(()),
        )
        .is_ok();
    if !base_exists || !head_exists {
        return Ok(None);
    }

    let base_cases = load_compare_cases(conn, base)?;
    let head_cases = load_compare_cases(conn, head)?;

    let mut keys: Vec<String> = base_cases.keys().cloned().collect();
    for k in head_cases.keys() {
        if !base_cases.contains_key(k) {
            keys.push(k.clone());
        }
    }
    keys.sort();

    let mut newly_failing = 0u64;
    let mut newly_passing = 0u64;
    let mut still_failing = 0u64;
    let mut output_changed = 0u64;
    let mut added = 0u64;
    let mut removed = 0u64;
    let mut cases = Vec::new();

    for key in keys {
        let b = base_cases.get(&key);
        let h = head_cases.get(&key);
        let (delta, out_changed) = match (b, h) {
            (Some(b), Some(h)) => {
                let out_changed = b.output_hash != h.output_hash;
                if out_changed {
                    output_changed += 1;
                }
                let delta = if b.status == CaseStatus::Pass && is_failing(h.status) {
                    newly_failing += 1;
                    "newly_failing"
                } else if is_failing(b.status) && h.status == CaseStatus::Pass {
                    newly_passing += 1;
                    "newly_passing"
                } else if is_failing(b.status) && is_failing(h.status) {
                    still_failing += 1;
                    "still_failing"
                } else if b.status == CaseStatus::Pass && h.status == CaseStatus::Pass {
                    "still_passing"
                } else {
                    "unchanged"
                };
                (delta, out_changed)
            }
            (None, Some(_)) => {
                added += 1;
                ("added", false)
            }
            (Some(_), None) => {
                removed += 1;
                ("removed", false)
            }
            (None, None) => unreachable!(),
        };
        let name = h
            .and_then(|c| c.name.clone())
            .or_else(|| b.and_then(|c| c.name.clone()));
        cases.push(serde_json::json!({
            "case_key": key,
            "name": name,
            "base_status": b.map(|c| c.status.as_str()),
            "head_status": h.map(|c| c.status.as_str()),
            "delta": delta,
            "output_changed": out_changed,
        }));
    }

    Ok(Some(serde_json::json!({
        "base": base,
        "head": head,
        "summary": {
            "newly_failing": newly_failing,
            "newly_passing": newly_passing,
            "still_failing": still_failing,
            "output_changed": output_changed,
            "added": added,
            "removed": removed,
        },
        "cases": cases,
    })))
}
