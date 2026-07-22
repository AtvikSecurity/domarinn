//! Case history: one case's evolution across the recent runs of a suite.
//!
//! Read-only. The same `case_key` identifies the same matrix cell in every run
//! of a suite (see [`domarinn_core::result::CellKey::case_key`]), so a single
//! indexed join over `cases`/`runs` walks that cell backwards in time. Powers
//! `GET /projects/{project}/suites/{suite}/cases/{case_key}/history`.

use std::str::FromStr;

use rusqlite::{params, Connection};

use domarinn_core::ids::{CaseKey, RunId};
use domarinn_core::result::CaseStatus;

use super::{empty_to_none, from_microusd, ms_to_rfc3339, Storage};
use crate::dto::history::{CaseHistoryPoint, CaseHistoryResponse};

impl Storage {
    /// A case's history across the last `limit` runs of `(project, suite)`,
    /// newest first. `None` when no run of that suite carries the `case_key`
    /// (an unknown case, project, or suite — the endpoint does not distinguish,
    /// all three surface as a 404).
    pub async fn case_history(
        &self,
        project: String,
        suite: String,
        case_key: CaseKey,
        limit: i64,
    ) -> anyhow::Result<Option<CaseHistoryResponse>> {
        self.runs
            .read(move |conn| case_history(conn, &project, &suite, &case_key, limit))
            .await
    }
}

fn case_history(
    conn: &Connection,
    project: &str,
    suite: &str,
    case_key: &CaseKey,
    limit: i64,
) -> anyhow::Result<Option<CaseHistoryResponse>> {
    let mut stmt = conn.prepare(
        "SELECT r.id, r.created_at, r.git_commit, r.config_digest,
                c.status, c.score, c.output_hash,
                c.prompt_tokens, c.completion_tokens, c.cost_microusd, c.latency_ms
         FROM cases c JOIN runs r ON r.id = c.run_id
         WHERE c.case_key = ?1 AND r.project = ?2 AND r.suite = ?3
         ORDER BY r.created_at DESC, r.id DESC
         LIMIT ?4",
    )?;
    let rows = stmt.query_map(params![case_key.as_str(), project, suite, limit], |row| {
        let status_raw: String = row.get(4)?;
        let status = CaseStatus::from_str(&status_raw).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(4, rusqlite::types::Type::Text, e.into())
        })?;
        Ok(CaseHistoryPoint {
            run_id: RunId::new(row.get::<_, String>(0)?),
            created_at: ms_to_rfc3339(row.get::<_, i64>(1)?),
            // Empty-string sentinels (and NULLs) map to `None` on the wire.
            git_commit: empty_to_none(row.get::<_, Option<String>>(2)?),
            config_digest: empty_to_none(row.get::<_, Option<String>>(3)?),
            status,
            score: row.get::<_, Option<f64>>(5)?,
            output_hash: row.get::<_, Option<String>>(6)?,
            // Filled in after collection: it depends on the neighbouring row.
            output_changed: None,
            prompt_tokens: row.get::<_, Option<i64>>(7)?,
            completion_tokens: row.get::<_, Option<i64>>(8)?,
            cost_usd: from_microusd(row.get::<_, Option<i64>>(9)?),
            latency_ms: row.get::<_, Option<i64>>(10)?,
        })
    })?;
    let mut points: Vec<CaseHistoryPoint> = rows.collect::<Result<_, _>>()?;

    // Zero rows means the case_key never appeared in any run of this
    // project/suite — a 404 at the handler.
    if points.is_empty() {
        return Ok(None);
    }

    // `output_changed` compares each point to the chronologically previous run.
    // Points are newest-first, so the next-older run is `points[i + 1]`; the
    // oldest returned point (and any point whose neighbour or itself has a NULL
    // hash) gets `None`. Computed into a side vector first to avoid aliasing the
    // slice while mutating it.
    let changes: Vec<Option<bool>> = (0..points.len())
        .map(|i| match points.get(i + 1) {
            Some(older) => match (
                points[i].output_hash.as_deref(),
                older.output_hash.as_deref(),
            ) {
                (Some(cur), Some(prev)) => Some(cur != prev),
                _ => None,
            },
            None => None,
        })
        .collect();
    for (point, changed) in points.iter_mut().zip(changes) {
        point.output_changed = changed;
    }

    let baseline_run_id: Option<String> = conn
        .query_row(
            "SELECT run_id FROM baselines WHERE project = ?1 AND suite = ?2",
            params![project, suite],
            |row| row.get(0),
        )
        .ok();

    Ok(Some(CaseHistoryResponse {
        project: project.to_string(),
        suite: suite.to_string(),
        case_key: case_key.clone(),
        baseline_run_id: baseline_run_id.map(RunId::new),
        points,
    }))
}
