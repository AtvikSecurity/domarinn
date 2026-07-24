//! One-shot, idempotent backfill of the migration-3 and migration-6 columns
//! from the stored zstd blobs.
//!
//! Migration 3 adds `provider_id`/`prompt_id`/`test_id`/`repeat_idx`/`score`/
//! `stop_reason` to `cases` and `config_digest` to `runs`; migration 6 adds
//! `cached` to `cases` and `cache_hits`/`cache_misses` to `runs`. A schema
//! migration cannot decode the compressed blobs to fill them, so pre-existing
//! rows land with those columns NULL. This runs once on every open (right after
//! `to_latest`) and populates any still-NULL rows from `cases.detail` and
//! `run_blobs.body`.
//!
//! It is idempotent by construction: the driving predicates are `provider_id IS
//! NULL OR cached IS NULL` / `config_digest IS NULL OR cache_hits IS NULL`, so
//! a fully-backfilled database (and every fresh database, whose rows are
//! written already-populated by ingest) selects zero rows and the loops exit
//! immediately. A row whose blob cannot be decompressed or parsed is stamped
//! with a sentinel (empty string on text columns, -1 on the numeric cache
//! columns) so it is never rescanned — the alternative (leaving it NULL) would
//! spin forever.

use anyhow::Context;
use rusqlite::{params, Connection, TransactionBehavior};

use domarinn_core::result::CaseResult;

use super::decompress;

/// Rows processed per `cases` chunk (one IMMEDIATE transaction each).
const CASE_CHUNK: i64 = 500;
/// Rows processed per `runs` chunk (blobs are larger; keep the chunk smaller).
const RUN_CHUNK: i64 = 100;

/// Run both backfills against the runs-database writer connection. Called from
/// `Storage::open_blocking` after migrations, so it borrows the writer
/// exclusively and does its work before any request can race it.
pub(super) fn run(conn: &mut Connection) -> anyhow::Result<()> {
    backfill_cases(conn)?;
    backfill_runs(conn)?;
    // FTS index for rows that predate migration 5 (see `storage::search`).
    super::search::backfill(conn)?;
    Ok(())
}

fn backfill_cases(conn: &mut Connection) -> anyhow::Result<()> {
    loop {
        // Read a chunk, then drop the statement before opening the write txn.
        let chunk: Vec<(i64, Option<Vec<u8>>)> = {
            let mut stmt = conn.prepare(
                "SELECT rowid, detail FROM cases
                 WHERE provider_id IS NULL OR cached IS NULL LIMIT ?1",
            )?;
            let rows = stmt.query_map(params![CASE_CHUNK], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, Option<Vec<u8>>>(1)?))
            })?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        if chunk.is_empty() {
            break;
        }
        let n = chunk.len();

        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for (rowid, detail) in chunk {
            match decode_case(detail.as_deref()) {
                Ok(case) => {
                    tx.execute(
                        "UPDATE cases
                         SET provider_id = ?1, prompt_id = ?2, test_id = ?3,
                             repeat_idx = ?4, score = ?5, stop_reason = ?6, cached = ?7
                         WHERE rowid = ?8",
                        params![
                            case.cell.provider_id,
                            case.cell.prompt_id,
                            case.cell.test_id,
                            case.cell.repeat as i64,
                            case.score,
                            case.stop_reason,
                            case.cached as i64,
                            rowid,
                        ],
                    )?;
                }
                Err(error) => {
                    tracing::warn!(
                        rowid,
                        error = %error,
                        "backfill: undecodable case detail; stamping sentinel"
                    );
                    tx.execute(
                        "UPDATE cases SET provider_id = '', cached = -1 WHERE rowid = ?1",
                        params![rowid],
                    )?;
                }
            }
        }
        tx.commit()?;
        tracing::debug!(cases = n, "backfill: repopulated cases chunk");
    }
    Ok(())
}

fn backfill_runs(conn: &mut Connection) -> anyhow::Result<()> {
    loop {
        let chunk: Vec<(String, Vec<u8>)> = {
            let mut stmt = conn.prepare(
                "SELECT run_id, body FROM run_blobs
                 WHERE run_id IN (
                     SELECT id FROM runs
                     WHERE config_digest IS NULL OR cache_hits IS NULL
                 )
                 LIMIT ?1",
            )?;
            let rows = stmt.query_map(params![RUN_CHUNK], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
            })?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        if chunk.is_empty() {
            break;
        }
        let n = chunk.len();

        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for (run_id, body) in chunk {
            match decode_run_columns(&body) {
                Ok((digest, cache_hits, cache_misses)) => {
                    tx.execute(
                        "UPDATE runs
                         SET config_digest = ?1, cache_hits = ?2, cache_misses = ?3
                         WHERE id = ?4",
                        params![digest, cache_hits, cache_misses, run_id],
                    )?;
                }
                Err(error) => {
                    tracing::warn!(
                        run_id = %run_id,
                        error = %error,
                        "backfill: undecodable run blob; stamping sentinel"
                    );
                    tx.execute(
                        "UPDATE runs
                         SET config_digest = '', cache_hits = -1, cache_misses = -1
                         WHERE id = ?1",
                        params![run_id],
                    )?;
                }
            }
        }
        tx.commit()?;
        tracing::debug!(runs = n, "backfill: repopulated runs chunk");
    }
    Ok(())
}

/// Decompress + parse a `cases.detail` blob into a [`CaseResult`]. The v2 struct
/// parses v1 blobs (the added fields are all optional).
fn decode_case(detail: Option<&[u8]>) -> anyhow::Result<CaseResult> {
    let detail = detail.context("case.detail is NULL")?;
    let bytes = decompress(detail).context("decompressing case.detail")?;
    let case = serde_json::from_slice::<CaseResult>(&bytes).context("parsing case.detail JSON")?;
    Ok(case)
}

/// Pull `config_digest` and the summary cache counters out of a
/// `run_blobs.body` blob without fully deserializing the (large) `RunResult`.
/// Blobs from before the counters existed default them to 0/0 — which the
/// run-list filter reads as "not fully cached", i.e. never hidden.
fn decode_run_columns(body: &[u8]) -> anyhow::Result<(String, i64, i64)> {
    let bytes = decompress(body).context("decompressing run body")?;
    let value = serde_json::from_slice::<serde_json::Value>(&bytes).context("parsing run JSON")?;
    let digest = value
        .get("config_digest")
        .and_then(|v| v.as_str())
        .context("run body missing config_digest")?;
    let summary = value.get("summary");
    let count = |key: &str| -> i64 {
        summary
            .and_then(|s| s.get(key))
            .and_then(|v| v.as_i64())
            .unwrap_or(0)
    };
    Ok((
        digest.to_string(),
        count("cache_hits"),
        count("cache_misses"),
    ))
}
