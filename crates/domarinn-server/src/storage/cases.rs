//! Case list (lean rows) and case detail (blob decompress).

use std::str::FromStr;

use domarinn_core::ids::{CaseKey, RunId};
use domarinn_core::result::CaseStatus;

use super::exec::{Conn, Queryable, Value};
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
    /// Exact-match on the promoted empty-output reason (migration 15). Exact
    /// only: the reason set is open, and unlike `error_class` there is no UI
    /// aggregation contract that needs an `unknown` bucket.
    pub empty_reason: Option<String>,
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
    fn query(self, conn: &mut Conn<'_>) -> anyhow::Result<CaseListResponse> {
        let mut sql = String::from(
            "SELECT case_key, idx, name, status, output_preview, asserts,
                    prompt_tokens, completion_tokens, cost_microusd, latency_ms,
                    provider_id, prompt_id, test_id, repeat_idx, score, stop_reason,
                    cached, error, error_class, empty_reason, answered_by_provider_id
             FROM cases WHERE run_id = ?1",
        );
        let mut args: Vec<Value> = vec![self.run_id.as_str().to_string().into()];
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
            // `LOWER` on both sides: SQLite's LIKE is ASCII-case-insensitive on
            // its own (making LOWER a no-op there), Postgres's is
            // case-sensitive — the explicit LOWER is what makes both engines
            // agree on a case-insensitive match.
            sql.push_str(&format!(
                " AND (LOWER(output_text) LIKE LOWER(?{a}) OR LOWER(name) LIKE LOWER(?{b}))"
            ));
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
            //
            // `= ''` rides along because the breakdown counts an empty-string
            // class under the same bucket (`ErrorClass` is unvalidated and an
            // exec child can emit `""`): without it the chip says `unknown × 1`
            // and clicking it filters to zero cases.
            if error_class == UNCLASSIFIED_ERROR {
                sql.push_str(" AND status = 'error' AND (error_class IS NULL OR error_class = '')");
            } else {
                args.push(error_class.clone().into());
                sql.push_str(&format!(" AND error_class = ?{}", args.len()));
            }
        }
        if let Some(empty_reason) = &self.empty_reason {
            args.push(empty_reason.clone().into());
            // `<> ''` is a no-op for every real reason and the whole point for a
            // blank one: `''` is storage's "known: not empty" sentinel, which
            // the row map unwraps to `None`, so it is never a value on the wire.
            // Without it, `?empty_reason=` selects the sentinel rows and returns
            // the exact complement of what was asked for.
            sql.push_str(&format!(
                " AND empty_reason = ?{} AND empty_reason <> ''",
                args.len()
            ));
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

        let mut collected: Vec<(i64, CaseListItem)> = conn.query_map(&sql, &args, |row| {
            let case_key: String = row.get(0)?;
            let asserts_str: Option<String> = row.get(5)?;
            // Graceful degrade (see `parse_stored_asserts`): a corrupt/tampered
            // row lists as having no asserts rather than failing the whole read.
            let asserts = parse_stored_asserts(asserts_str, self.run_id.as_str(), &case_key);
            let status_raw: String = row.get(3)?;
            let status =
                CaseStatus::from_str(&status_raw).map_err(|e| anyhow::anyhow!("column 3: {e}"))?;
            let idx: i64 = row.get(1)?;
            Ok((
                idx,
                CaseListItem {
                    case_key: CaseKey::new(row.get::<String>(0)?),
                    idx,
                    name: row.get::<Option<String>>(2)?,
                    status,
                    output_preview: row.get::<Option<String>>(4)?,
                    asserts,
                    prompt_tokens: row.get::<Option<i64>>(6)?,
                    completion_tokens: row.get::<Option<i64>>(7)?,
                    cost_usd: from_microusd(row.get::<Option<i64>>(8)?),
                    latency_ms: row.get::<Option<i64>>(9)?,
                    // Map the empty-string backfill sentinel to `None` on the
                    // text columns; `repeat_idx`/`score` are numeric and land
                    // NULL on sentinel rows.
                    provider_id: empty_to_none(row.get::<Option<String>>(10)?),
                    prompt_id: empty_to_none(row.get::<Option<String>>(11)?),
                    test_id: empty_to_none(row.get::<Option<String>>(12)?),
                    repeat: row.get::<Option<i64>>(13)?,
                    score: row.get::<Option<f64>>(14)?,
                    stop_reason: empty_to_none(row.get::<Option<String>>(15)?),
                    // 1/0 → Some(true/false); NULL (legacy) and the -1
                    // undecodable-blob sentinel → None.
                    cached: row.get::<Option<i64>>(16)?.and_then(|v| match v {
                        1 => Some(true),
                        0 => Some(false),
                        _ => None,
                    }),
                    // '' means "known: no error"; NULL means "not backfilled
                    // yet". Both read as `None` on the wire.
                    error: empty_to_none(row.get::<Option<String>>(17)?),
                    error_class: row.get::<Option<String>>(18)?,
                    // Same tri-state as `error`: '' is "known: not empty",
                    // NULL is "not backfilled yet".
                    empty_reason: empty_to_none(row.get::<Option<String>>(19)?),
                    // And once more: '' is "the configured provider answered",
                    // NULL is "not backfilled yet".
                    answered_by_provider_id: empty_to_none(row.get::<Option<String>>(20)?),
                },
            ))
        })?;

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
    conn: &mut Conn<'_>,
    run_id: &RunId,
    case_key: &CaseKey,
    vis: &RunVisibility,
) -> anyhow::Result<Option<CaseDetailResponse>> {
    let mut args: Vec<Value> = vec![
        run_id.as_str().to_string().into(),
        case_key.as_str().to_string().into(),
    ];
    let visible = visible_run_predicate(1, vis, &mut args);
    let sql = format!("SELECT detail FROM cases WHERE run_id = ?1 AND case_key = ?2 AND {visible}");
    // `query_row_opt`, never `.ok()` — see `project_has_visible_runs` in
    // [`super::sets`]. This answer becomes a 404, and a storage fault must not
    // be reported to the caller as a case that does not exist.
    let blob: Option<Vec<u8>> = conn.query_row_opt(&sql, &args, |row| row.get(0))?;
    let Some(blob) = blob else {
        return Ok(None);
    };
    let bytes = decompress(&blob)?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)?;
    Ok(Some(CaseDetailResponse(value)))
}
