//! Which runs used a given cache entry.
//!
//! This spans both databases by *identity*, not by JOIN. `runs` and `cache` are
//! separate files with separate connections and separate writer mutexes, so a
//! cross-database join is neither possible nor needed: the key arrives from the
//! URL, and everything the answer requires is in the runs database.
//!
//! # What absence means here
//!
//! A case only carries a key if it was recorded by a version that knew to write
//! one, and no backfill can supply the rest — the key is derived from the
//! canonical request plus the repeat index and salts, and a stored run document
//! holds none of those. So an entry with no runs is genuinely ambiguous
//! ("nothing used it" or "everything that used it predates the column") and
//! every reader has to say so rather than implying the entry is unused.

use rusqlite::{params, Connection};

use domarinn_core::ids::RunId;

use super::{decode_cursor, encode_cursor, ms_to_rfc3339, Storage};
use crate::dto::cacheentries::{CacheEntryRunRef, CacheEntryRunsResponse};

impl Storage {
    /// Cases whose provider call was addressed by `key`, newest run first.
    pub async fn cache_entry_runs(
        &self,
        key: String,
        limit: i64,
        cursor: Option<(i64, RunId)>,
    ) -> anyhow::Result<CacheEntryRunsResponse> {
        self.runs
            .read(move |conn| entry_runs(conn, &key, limit, cursor))
            .await
    }
}

fn entry_runs(
    conn: &Connection,
    key: &str,
    limit: i64,
    cursor: Option<(i64, RunId)>,
) -> anyhow::Result<CacheEntryRunsResponse> {
    let mut sql = String::from(
        "SELECT runs.id, runs.project, runs.suite, runs.created_at,
                cases.case_key, cases.name, cases.status, cases.cached
           FROM cases
           JOIN runs ON runs.id = cases.run_id
          WHERE cases.cache_key = ?1",
    );
    // Same keyset shape as the runs list, and its cursor helper is reusable
    // here because this page really is run-shaped.
    if cursor.is_some() {
        sql.push_str(" AND (runs.created_at < ?3 OR (runs.created_at = ?3 AND runs.id < ?4))");
    }
    sql.push_str(" ORDER BY runs.created_at DESC, runs.id DESC LIMIT ?2");

    let mut stmt = conn.prepare(&sql)?;
    let map = |row: &rusqlite::Row<'_>| -> rusqlite::Result<(CacheEntryRunRef, i64, String)> {
        let created_at: i64 = row.get(3)?;
        let run_id: String = row.get(0)?;
        Ok((
            CacheEntryRunRef {
                run_id: run_id.clone(),
                project: row.get(1)?,
                suite: row.get(2)?,
                created_at: ms_to_rfc3339(created_at),
                case_key: row.get(4)?,
                name: row.get(5)?,
                status: row.get(6)?,
                cached: row.get::<_, Option<i64>>(7)?.unwrap_or(0) != 0,
            },
            created_at,
            run_id,
        ))
    };
    // Fetch one extra to learn whether a next page exists without counting.
    let fetch = limit + 1;
    let rows: Vec<(CacheEntryRunRef, i64, String)> = match &cursor {
        Some((created_at, run_id)) => stmt
            .query_map(params![key, fetch, created_at, run_id.as_str()], map)?
            .collect::<Result<_, _>>()?,
        None => stmt
            .query_map(params![key, fetch], map)?
            .collect::<Result<_, _>>()?,
    };

    let mut rows = rows;
    let next_cursor = if rows.len() as i64 > limit {
        rows.truncate(limit as usize);
        rows.last()
            .map(|(_, created_at, run_id)| encode_cursor(*created_at, &RunId::new(run_id)))
    } else {
        None
    };

    Ok(CacheEntryRunsResponse {
        cases: rows.into_iter().map(|(item, _, _)| item).collect(),
        next_cursor,
    })
}

/// Decode a run-shaped cursor for this endpoint.
pub fn decode_run_cursor(cursor: &str) -> Option<(i64, RunId)> {
    decode_cursor(cursor)
}
