//! Per-run prompt × provider aggregate matrix.
//!
//! Read-only. One column-only scan of a run's `cases` (no blob loads) is
//! aggregated in Rust into the wire shape the UI's matrix view consumes: rows
//! are tests, columns are distinct `(provider, prompt)` pairs, and each cell
//! collapses every repeat of that test × column into status counts plus
//! flakiness signals. Powers `GET /runs/{id}/matrix`.
//!
//! Columns are always the complete set for the run (they are small); only the
//! test rows paginate, over their first-seen `idx` boundary — the same
//! opaque-cursor style the case-list endpoint uses.

use std::collections::{HashMap, HashSet};
use std::str::FromStr;

use rusqlite::{params, Connection};

use domarinn_core::ids::{CaseKey, RunId};
use domarinn_core::result::CaseStatus;

use super::{empty_to_none, from_microusd, Storage};
use crate::dto::matrix::{MatrixCell, MatrixColumn, MatrixResponse, MatrixRow};

impl Storage {
    /// Aggregate a run's cases into the prompt × provider matrix, paginating the
    /// test rows. The run's existence is checked by the handler (mirroring the
    /// case-list endpoint), so a run with no cell-bearing cases returns an empty
    /// matrix rather than an error.
    pub async fn run_matrix(&self, filter: MatrixFilter) -> anyhow::Result<MatrixResponse> {
        self.runs.read(move |conn| filter.query(conn)).await
    }
}

/// Inputs for `GET /runs/{id}/matrix`.
#[derive(Debug, Clone)]
pub struct MatrixFilter {
    pub run_id: RunId,
    /// Max test ROWS per page (columns never paginate).
    pub limit: i64,
    /// First-seen `idx` boundary: return rows first seen strictly after it.
    pub cursor: Option<i64>,
}

/// One `cases` row projected for aggregation. `idx`/`repeat_idx` are carried
/// beyond the DTO shape purely to order rows (first-seen `idx`) and a cell's
/// case keys (`repeat_idx`, ties by `idx`).
struct RawCase {
    test_id: String,
    provider_id: String,
    prompt_id: Option<String>,
    name: Option<String>,
    status: CaseStatus,
    score: Option<f64>,
    output_hash: Option<String>,
    latency_ms: Option<i64>,
    cost_microusd: Option<i64>,
    case_key: String,
    repeat_idx: i64,
    idx: i64,
}

/// Accumulator for one test × column cell before it is finalized.
#[derive(Default)]
struct CellAcc {
    total: i64,
    passed: i64,
    failed: i64,
    errored: i64,
    skipped: i64,
    score_sum: f64,
    score_count: i64,
    output_hashes: HashSet<String>,
    latency_sum: i64,
    latency_count: i64,
    cost_micro_sum: i64,
    cost_any: bool,
    /// `(repeat_idx, idx, case_key)` — sorted at finalize time.
    case_keys: Vec<(i64, i64, String)>,
}

/// Accumulator for one test row: its cells keyed by column index.
struct RowAcc {
    test_id: String,
    first_seen_idx: i64,
    name: Option<String>,
    cells: HashMap<usize, CellAcc>,
}

impl MatrixFilter {
    fn query(self, conn: &Connection) -> anyhow::Result<MatrixResponse> {
        // Column-only scan, insertion order. `idx`/`repeat_idx` come along for
        // row/case-key ordering. Legacy/failed-backfill rows (NULL or `''`
        // provider_id) are excluded here, so a pre-backfill run scans to zero
        // rows and returns an empty matrix.
        let mut stmt = conn.prepare(
            "SELECT test_id, provider_id, prompt_id, name, status, score,
                    output_hash, latency_ms, cost_microusd, case_key, repeat_idx, idx
             FROM cases
             WHERE run_id = ?1 AND provider_id IS NOT NULL AND provider_id != ''
             ORDER BY idx",
        )?;
        let rows = stmt.query_map(params![self.run_id.as_str()], |row| {
            let status_raw: String = row.get(4)?;
            let status = CaseStatus::from_str(&status_raw).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(4, rusqlite::types::Type::Text, e.into())
            })?;
            Ok(RawCase {
                // `test_id` maps the `''` sentinel to `None`; such a row has a
                // provider but no test identity, so it cannot form a matrix
                // cell and is dropped below.
                test_id: empty_to_none(row.get::<_, Option<String>>(0)?).unwrap_or_default(),
                provider_id: row.get::<_, String>(1)?,
                prompt_id: empty_to_none(row.get::<_, Option<String>>(2)?),
                name: row.get::<_, Option<String>>(3)?,
                status,
                score: row.get::<_, Option<f64>>(5)?,
                output_hash: row.get::<_, Option<String>>(6)?,
                latency_ms: row.get::<_, Option<i64>>(7)?,
                cost_microusd: row.get::<_, Option<i64>>(8)?,
                case_key: row.get::<_, String>(9)?,
                repeat_idx: row.get::<_, Option<i64>>(10)?.unwrap_or(0),
                idx: row.get::<_, i64>(11)?,
            })
        })?;

        // First-seen ordered column set: `(provider_id, prompt_id)` -> index.
        let mut column_index: HashMap<(String, Option<String>), usize> = HashMap::new();
        let mut columns: Vec<MatrixColumn> = Vec::new();
        // First-seen ordered rows: `test_id` -> index into `row_accs`.
        let mut row_index: HashMap<String, usize> = HashMap::new();
        let mut row_accs: Vec<RowAcc> = Vec::new();

        for raw in rows {
            let raw = raw?;
            // A provider-bearing row with no test identity can't be placed.
            if raw.test_id.is_empty() {
                continue;
            }

            let col_key = (raw.provider_id.clone(), raw.prompt_id.clone());
            let col = *column_index.entry(col_key).or_insert_with(|| {
                columns.push(MatrixColumn {
                    provider_id: raw.provider_id.clone(),
                    prompt_id: raw.prompt_id.clone(),
                });
                columns.len() - 1
            });

            let row_pos = *row_index.entry(raw.test_id.clone()).or_insert_with(|| {
                row_accs.push(RowAcc {
                    test_id: raw.test_id.clone(),
                    first_seen_idx: raw.idx,
                    name: None,
                    cells: HashMap::new(),
                });
                row_accs.len() - 1
            });
            let row = &mut row_accs[row_pos];
            // First non-null name seen for this test wins.
            if row.name.is_none() {
                if let Some(name) = &raw.name {
                    row.name = Some(name.clone());
                }
            }

            let cell = row.cells.entry(col).or_default();
            cell.total += 1;
            match raw.status {
                CaseStatus::Pass => cell.passed += 1,
                CaseStatus::Fail => cell.failed += 1,
                CaseStatus::Error => cell.errored += 1,
                CaseStatus::Skip => cell.skipped += 1,
            }
            if let Some(score) = raw.score {
                cell.score_sum += score;
                cell.score_count += 1;
            }
            if let Some(hash) = raw.output_hash {
                cell.output_hashes.insert(hash);
            }
            if let Some(latency) = raw.latency_ms {
                cell.latency_sum += latency;
                cell.latency_count += 1;
            }
            if let Some(cost) = raw.cost_microusd {
                cell.cost_micro_sum += cost;
                cell.cost_any = true;
            }
            cell.case_keys.push((raw.repeat_idx, raw.idx, raw.case_key));
        }

        let total_columns = columns.len();

        // Rows are already in first-seen order (the scan is `ORDER BY idx` and
        // each test is pushed on first encounter). Paginate over the first-seen
        // `idx` boundary, mirroring the case-list cursor: keep rows first seen
        // after the cursor, take `limit`, and expose a `next_cursor` only when
        // more rows remain.
        let after_cursor = row_accs
            .into_iter()
            .filter(|r| self.cursor.is_none_or(|c| r.first_seen_idx > c));
        let mut page: Vec<RowAcc> = Vec::new();
        let mut has_more = false;
        for row in after_cursor {
            if page.len() as i64 >= self.limit {
                has_more = true;
                break;
            }
            page.push(row);
        }
        let next_cursor = if has_more {
            page.last().map(|r| r.first_seen_idx.to_string())
        } else {
            None
        };

        let rows: Vec<MatrixRow> = page
            .into_iter()
            .map(|row| finalize_row(row, total_columns))
            .collect();

        Ok(MatrixResponse {
            run_id: self.run_id,
            columns,
            rows,
            next_cursor,
        })
    }
}

/// Turn a row accumulator into its wire shape, aligning cells 1:1 with the
/// complete column set (a `None` where the test never ran on that column).
fn finalize_row(row: RowAcc, total_columns: usize) -> MatrixRow {
    let mut cells: Vec<Option<MatrixCell>> = (0..total_columns).map(|_| None).collect();
    for (col, acc) in row.cells {
        cells[col] = Some(finalize_cell(acc));
    }
    MatrixRow {
        test_id: row.test_id,
        name: row.name,
        cells,
    }
}

fn finalize_cell(acc: CellAcc) -> MatrixCell {
    let mut case_keys = acc.case_keys;
    // Order by `repeat_idx`, ties broken by `idx`.
    case_keys.sort_by_key(|(repeat, idx, _)| (*repeat, *idx));
    let case_keys = case_keys
        .into_iter()
        .map(|(_, _, key)| CaseKey::new(key))
        .collect();

    let score_mean = if acc.score_count > 0 {
        Some(acc.score_sum / acc.score_count as f64)
    } else {
        None
    };
    let latency_ms_mean = if acc.latency_count > 0 {
        Some(acc.latency_sum as f64 / acc.latency_count as f64)
    } else {
        None
    };
    let cost_usd = if acc.cost_any {
        from_microusd(Some(acc.cost_micro_sum))
    } else {
        None
    };
    // A finalized cell always has at least one repeat, so `total >= 1`.
    let pass_fraction = acc.passed as f64 / acc.total as f64;

    MatrixCell {
        total: acc.total,
        passed: acc.passed,
        failed: acc.failed,
        errored: acc.errored,
        skipped: acc.skipped,
        score_mean,
        pass_fraction,
        distinct_outputs: acc.output_hashes.len() as i64,
        latency_ms_mean,
        cost_usd,
        case_keys,
    }
}
