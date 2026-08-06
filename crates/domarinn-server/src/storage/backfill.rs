//! One-shot, idempotent backfill of the migration-3, -6, -7, -15 and -17
//! columns from the stored zstd blobs.
//!
//! Migration 3 adds `provider_id`/`prompt_id`/`test_id`/`repeat_idx`/`score`/
//! `stop_reason` to `cases` and `config_digest` to `runs`; migration 6 adds
//! `cached` to `cases` and `cache_hits`/`cache_misses` to `runs`; migration 7
//! adds `error` to `cases`; migration 15 adds `empty_reason` to `cases` and
//! `empty_count` to `runs`; migration 17 adds `answered_by_provider_id` to
//! `cases`. A schema
//! migration cannot decode the compressed blobs to fill them, so pre-existing
//! rows land with those columns NULL. This runs once on every open (right after
//! `to_latest`) and populates any still-NULL rows from `cases.detail` and
//! `run_blobs.body`.
//!
//! It is idempotent by construction: the driving predicates are `provider_id IS
//! NULL OR cached IS NULL OR error IS NULL OR empty_reason IS NULL OR
//! answered_by_provider_id IS NULL` /
//! `config_digest IS NULL OR cache_hits IS NULL OR empty_count IS NULL`, so
//! a fully-backfilled database (and every fresh database, whose rows are
//! written already-populated by ingest) selects zero rows and the loops exit
//! immediately. A row whose blob cannot be decompressed or parsed is stamped
//! with a sentinel (empty string on text columns, -1 on the numeric cache and
//! empty counters) so it is never rescanned — the alternative (leaving it NULL)
//! would spin forever. `error`, `empty_reason` and `answered_by_provider_id`
//! are tri-states for the same reason: NULL means "not yet backfilled", and a
//! case with no error (no empty reason, no fallback answerer) is stamped `''`
//! rather than left NULL, which would make it eligible for every subsequent
//! pass.
//!
//! A blob written before `empty_reason` existed honestly backfills to `''` per
//! case and `0` for the run: the field is absent from the document, so no case
//! in it carried one. That is the right answer, not a gap. The same holds for
//! `answered_by_provider_id`: a blob predating provider fallback records no
//! handoff because none happened, so `''` ("the configured provider answered")
//! is the truth about it.
//!
//! A run missing *only* `empty_count` — the shape every run in the store has on
//! the first open after the migration-15 upgrade — is filled by one SQL COUNT
//! over its (already-backfilled) case rows instead of a blob decode; see
//! `backfill_runs`.

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
                 WHERE provider_id IS NULL OR cached IS NULL OR error IS NULL
                    OR empty_reason IS NULL OR answered_by_provider_id IS NULL
                 LIMIT ?1",
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
                             repeat_idx = ?4, score = ?5, stop_reason = ?6, cached = ?7,
                             error = ?8, empty_reason = ?9,
                             answered_by_provider_id = ?10
                         WHERE rowid = ?11",
                        params![
                            case.cell.provider_id,
                            case.cell.prompt_id,
                            case.cell.test_id,
                            case.cell.repeat as i64,
                            case.score,
                            case.stop_reason,
                            case.cached as i64,
                            // '' = "decoded, and there was no error" — distinct
                            // from NULL, which means "not yet looked at".
                            case.error.as_deref().unwrap_or_default(),
                            // Same tri-state, same `''`: a blob predating
                            // `empty_reason` has no field, and "this case's
                            // output was not empty" is the honest reading.
                            case.empty_reason
                                .as_ref()
                                .map(|r| r.as_str())
                                .unwrap_or_default(),
                            // Same tri-state once more: a blob predating
                            // provider fallback carries no answerer, and "the
                            // configured provider answered" is the honest
                            // reading of that absence.
                            case.answered_by_provider_id.as_deref().unwrap_or_default(),
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
                        "UPDATE cases SET provider_id = '', cached = -1, error = '',
                             empty_reason = '', answered_by_provider_id = ''
                         WHERE rowid = ?1",
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
    // Fast path for the migration-15 upgrade shape: every column but
    // `empty_count` already populated — which on the first open after that
    // upgrade is *every run in the store*, so letting these fall through to
    // the loop below would zstd-decode every run blob to fill one countable
    // column and hold startup for minutes on a large database. This depends
    // on `backfill_cases` having already run (see `run`): `cases.empty_reason`
    // is fully stamped by then, the migration-15 index covers the correlated
    // COUNT, and a count derived from the case rows agrees with the detail
    // view's per-reason GROUP BY by construction — the blob could disagree
    // with both. Runs carrying an undecodable case row (`cached = -1` is that
    // sentinel) are left to the blob path: their case rows no longer know the
    // truth, but the run blob still might.
    let fast = conn.execute(
        "UPDATE runs
         SET empty_count = (
             SELECT COUNT(*) FROM cases
             WHERE cases.run_id = runs.id AND cases.empty_reason <> ''
         )
         WHERE empty_count IS NULL
           AND config_digest IS NOT NULL
           AND cache_hits IS NOT NULL
           AND NOT EXISTS (
               SELECT 1 FROM cases
               WHERE cases.run_id = runs.id AND cases.cached = -1
           )",
        [],
    )?;
    if fast > 0 {
        tracing::debug!(runs = fast, "backfill: counted empty cases from case rows");
    }
    loop {
        let chunk: Vec<(String, Vec<u8>)> = {
            let mut stmt = conn.prepare(
                "SELECT run_id, body FROM run_blobs
                 WHERE run_id IN (
                     SELECT id FROM runs
                     WHERE config_digest IS NULL OR cache_hits IS NULL
                        OR empty_count IS NULL
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
                Ok(RunColumns {
                    digest,
                    cache_hits,
                    cache_misses,
                    empty_count,
                }) => {
                    tx.execute(
                        "UPDATE runs
                         SET config_digest = ?1, cache_hits = ?2, cache_misses = ?3,
                             empty_count = ?4
                         WHERE id = ?5",
                        params![digest, cache_hits, cache_misses, empty_count, run_id],
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
                         SET config_digest = '', cache_hits = -1, cache_misses = -1,
                             empty_count = -1
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

/// The promoted `runs` columns one blob yields.
struct RunColumns {
    digest: String,
    cache_hits: i64,
    cache_misses: i64,
    empty_count: i64,
}

/// Pull `config_digest`, the summary cache counters and the empty-case tally
/// out of a `run_blobs.body` blob without fully deserializing the (large)
/// `RunResult`. Blobs from before the counters existed default them to 0/0 —
/// which the run-list filter reads as "not fully cached", i.e. never hidden.
fn decode_run_columns(body: &[u8]) -> anyhow::Result<RunColumns> {
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
    // Counted off the cases rather than summed from `summary.empty_counts`, so
    // a blob written by a client that records the per-case reason but not the
    // summary tally still lands the right number. A blob predating the field
    // entirely counts 0, which is the truth about it. A present-but-empty
    // reason does not count either: it stores as the `''` "known: not empty"
    // sentinel, which the detail tally and the case grid both exclude, so
    // counting it here would make the list disagree with both.
    let empty_count = value
        .get("cases")
        .and_then(|v| v.as_array())
        .map(|cases| {
            cases
                .iter()
                .filter(|c| {
                    c.get("empty_reason")
                        .and_then(|r| r.as_str())
                        .is_some_and(|r| !r.is_empty())
                })
                .count() as i64
        })
        .unwrap_or(0);
    Ok(RunColumns {
        digest: digest.to_string(),
        cache_hits: count("cache_hits"),
        cache_misses: count("cache_misses"),
        empty_count,
    })
}
