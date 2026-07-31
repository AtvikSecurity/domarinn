//! Case list (lean rows) and case detail (blob decompress).

use std::str::FromStr;

use rusqlite::{Connection, OptionalExtension};

use domarinn_core::ids::{CaseKey, RunId};
use domarinn_core::result::CaseStatus;

use super::{decompress, empty_to_none, from_microusd, parse_stored_asserts, Storage};
use crate::dto::cases::{CaseDetailResponse, CaseListItem, CaseListResponse};
use crate::runsets::{visible_run_predicate, RunVisibility};

impl Storage {
    pub async fn list_cases(&self, filter: CaseListFilter) -> anyhow::Result<CaseListResponse> {
        self.runs.read(move |conn| filter.query(conn)).await
    }

    /// One case's detail, or `None` when the case, the run, or the caller's
    /// access to the run is missing — all three render as the same 404.
    pub async fn get_case(
        &self,
        run_id: RunId,
        case_key: CaseKey,
        vis: RunVisibility,
    ) -> anyhow::Result<Option<CaseDetailResponse>> {
        self.runs
            .read(move |conn| get_case_detail(conn, &run_id, &case_key, &vis))
            .await
    }
}

/// Filters for `GET /runs/{id}/cases`.
#[derive(Debug, Clone)]
pub struct CaseListFilter {
    pub run_id: RunId,
    /// Required, with no default: a case list is a view onto its run, so it
    /// answers to the same access check the run itself does.
    pub visibility: RunVisibility,
    pub status: Option<CaseStatus>,
    pub tag: Option<String>,
    pub q: Option<String>,
    /// Exact-match on the promoted cell columns (migration 3).
    pub provider: Option<String>,
    pub prompt: Option<String>,
    pub test: Option<String>,
    pub stop_reason: Option<String>,
    /// Exact-match on the promoted failure class (migration 10).
    pub error_class: Option<String>,
    /// `Some(false)` = fresh (non-cached) responses only; `Some(true)` =
    /// cache hits only. Legacy rows (NULL/sentinel `cached`) count as fresh —
    /// never hide what we can't classify.
    pub cached: Option<bool>,
    pub limit: i64,
    pub cursor: Option<i64>,
}

/// The wire value for "errored, but from a run written before error classes
/// existed". Shared with the web UI's `aggregateErrorClasses`, which buckets
/// unclassified errors under the same string so the breakdown's counts still
/// add up to the run's error count.
pub const UNCLASSIFIED_ERROR: &str = "unknown";

impl CaseListFilter {
    fn query(self, conn: &Connection) -> anyhow::Result<CaseListResponse> {
        let mut sql = String::from(
            "SELECT case_key, idx, name, status, output_preview, asserts,
                    prompt_tokens, completion_tokens, cost_microusd, latency_ms,
                    provider_id, prompt_id, test_id, repeat_idx, score, stop_reason,
                    cached, error, error_class
             FROM cases WHERE run_id = ?1",
        );
        let mut args: Vec<rusqlite::types::Value> = vec![self.run_id.as_str().to_string().into()];
        let visible = visible_run_predicate(1, &self.visibility, &mut args);
        sql.push_str(&format!(" AND {visible}"));
        if let Some(status) = self.status {
            args.push(status.as_str().to_string().into());
            sql.push_str(&format!(" AND status = ?{}", args.len()));
        }
        if let Some(tag) = &self.tag {
            args.push(tag.clone().into());
            sql.push_str(&format!(
                " AND case_key IN (SELECT case_key FROM case_tags WHERE run_id = ?1 AND tag = ?{})",
                args.len()
            ));
        }
        if let Some(q) = &self.q {
            let like = format!("%{}%", q);
            args.push(like.clone().into());
            let a = args.len();
            args.push(like.into());
            let b = args.len();
            sql.push_str(&format!(" AND (output_text LIKE ?{a} OR name LIKE ?{b})"));
        }
        if let Some(provider) = &self.provider {
            args.push(provider.clone().into());
            sql.push_str(&format!(" AND provider_id = ?{}", args.len()));
        }
        if let Some(prompt) = &self.prompt {
            args.push(prompt.clone().into());
            sql.push_str(&format!(" AND prompt_id = ?{}", args.len()));
        }
        if let Some(test) = &self.test {
            args.push(test.clone().into());
            sql.push_str(&format!(" AND test_id = ?{}", args.len()));
        }
        if let Some(stop_reason) = &self.stop_reason {
            args.push(stop_reason.clone().into());
            sql.push_str(&format!(" AND stop_reason = ?{}", args.len()));
        }
        if let Some(error_class) = &self.error_class {
            // `unknown` is the UI's bucket for cases that errored before this
            // column existed — every run predating migration 10, which is not
            // backfilled. Those rows hold NULL, and `NULL = 'unknown'` is NULL,
            // so without this branch the breakdown offers a filter that always
            // returns nothing.
            if error_class == UNCLASSIFIED_ERROR {
                sql.push_str(" AND status = 'error' AND error_class IS NULL");
            } else {
                args.push(error_class.clone().into());
                sql.push_str(&format!(" AND error_class = ?{}", args.len()));
            }
        }
        match self.cached {
            // Cache hits are exactly the rows stamped 1; NULL (legacy) and the
            // -1 sentinel are "unknown" and count as fresh on the other branch.
            Some(true) => sql.push_str(" AND cached = 1"),
            Some(false) => sql.push_str(" AND COALESCE(cached, 0) <> 1"),
            None => {}
        }
        if let Some(cursor) = self.cursor {
            args.push(cursor.into());
            sql.push_str(&format!(" AND idx > ?{}", args.len()));
        }
        args.push((self.limit + 1).into());
        sql.push_str(&format!(" ORDER BY idx ASC LIMIT ?{}", args.len()));

        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(args.iter()), |row| {
            let case_key: String = row.get(0)?;
            let asserts_str: Option<String> = row.get(5)?;
            // Graceful degrade (see `parse_stored_asserts`): a corrupt/tampered
            // row lists as having no asserts rather than failing the whole read.
            let asserts = parse_stored_asserts(asserts_str, self.run_id.as_str(), &case_key);
            let status_raw: String = row.get(3)?;
            let status = CaseStatus::from_str(&status_raw).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, e.into())
            })?;
            let idx: i64 = row.get(1)?;
            Ok((
                idx,
                CaseListItem {
                    case_key: CaseKey::new(row.get::<_, String>(0)?),
                    idx,
                    name: row.get::<_, Option<String>>(2)?,
                    status,
                    output_preview: row.get::<_, Option<String>>(4)?,
                    asserts,
                    prompt_tokens: row.get::<_, Option<i64>>(6)?,
                    completion_tokens: row.get::<_, Option<i64>>(7)?,
                    cost_usd: from_microusd(row.get::<_, Option<i64>>(8)?),
                    latency_ms: row.get::<_, Option<i64>>(9)?,
                    // Map the empty-string backfill sentinel to `None` on the
                    // text columns; `repeat_idx`/`score` are numeric and land
                    // NULL on sentinel rows.
                    provider_id: empty_to_none(row.get::<_, Option<String>>(10)?),
                    prompt_id: empty_to_none(row.get::<_, Option<String>>(11)?),
                    test_id: empty_to_none(row.get::<_, Option<String>>(12)?),
                    repeat: row.get::<_, Option<i64>>(13)?,
                    score: row.get::<_, Option<f64>>(14)?,
                    stop_reason: empty_to_none(row.get::<_, Option<String>>(15)?),
                    // 1/0 → Some(true/false); NULL (legacy) and the -1
                    // undecodable-blob sentinel → None.
                    cached: row.get::<_, Option<i64>>(16)?.and_then(|v| match v {
                        1 => Some(true),
                        0 => Some(false),
                        _ => None,
                    }),
                    // '' means "known: no error"; NULL means "not backfilled
                    // yet". Both read as `None` on the wire.
                    error: empty_to_none(row.get::<_, Option<String>>(17)?),
                    error_class: row.get::<_, Option<String>>(18)?,
                },
            ))
        })?;
        let mut collected: Vec<(i64, CaseListItem)> = Vec::new();
        for row in rows {
            collected.push(row?);
        }

        let mut next_cursor = None;
        if collected.len() as i64 > self.limit {
            collected.pop();
            if let Some((idx, _)) = collected.last() {
                next_cursor = Some(idx.to_string());
            }
        }

        let cases: Vec<CaseListItem> = collected.into_iter().map(|(_, v)| v).collect();
        Ok(CaseListResponse { cases, next_cursor })
    }
}

fn get_case_detail(
    conn: &Connection,
    run_id: &RunId,
    case_key: &CaseKey,
    vis: &RunVisibility,
) -> anyhow::Result<Option<CaseDetailResponse>> {
    let mut args: Vec<rusqlite::types::Value> = vec![
        run_id.as_str().to_string().into(),
        case_key.as_str().to_string().into(),
    ];
    let visible = visible_run_predicate(1, vis, &mut args);
    let sql = format!("SELECT detail FROM cases WHERE run_id = ?1 AND case_key = ?2 AND {visible}");
    // `.optional()?`, never `.ok()` — see `project_has_visible_runs` in
    // [`super::sets`]. This answer becomes a 404, and a storage fault must not
    // be reported to the caller as a case that does not exist.
    let blob: Option<Vec<u8>> = conn
        .query_row(&sql, rusqlite::params_from_iter(args.iter()), |row| {
            row.get(0)
        })
        .optional()?;
    let Some(blob) = blob else {
        return Ok(None);
    };
    let bytes = decompress(&blob)?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)?;
    Ok(Some(CaseDetailResponse(value)))
}
