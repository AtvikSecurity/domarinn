//! One-shot, idempotent backfill of the migration-3, -6, -7, -15, -17 and -18
//! columns from the stored zstd blobs.
//!
//! Migration 3 adds `provider_id`/`prompt_id`/`test_id`/`repeat_idx`/`score`/
//! `stop_reason` to `cases` and `config_digest` to `runs`; migration 6 adds
//! `cached` to `cases` and `cache_hits`/`cache_misses` to `runs`; migration 7
//! adds `error` to `cases`; migration 15 adds `empty_reason` to `cases` and
//! `empty_count` to `runs`; migration 17 adds `answered_by_provider_id` to
//! `cases` and migration 18 its run-level tally `fallback_count`. A schema
//! migration cannot decode the compressed blobs to fill them, so pre-existing
//! rows land with those columns NULL. This runs once on every open (right after
//! `to_latest`) and populates any still-NULL rows from `cases.detail` and
//! `run_blobs.body`.
//!
//! It is idempotent by construction: every driving predicate is "some column is
//! still NULL", so a fully-backfilled database (and every fresh database, whose
//! rows are written already-populated by ingest) selects zero rows and the
//! loops exit immediately. A row whose blob cannot be decompressed or parsed is
//! stamped with a sentinel (empty string on text columns, -1 on the numeric
//! cache, empty and fallback counters) so it is never rescanned — the
//! alternative (leaving it NULL) would spin forever. `error`, `empty_reason`
//! and `answered_by_provider_id` are tri-states for the same reason: NULL means
//! "not yet backfilled", and a case with no error (no empty reason, no fallback
//! answerer) is stamped `''` rather than left NULL, which would make it
//! eligible for every subsequent pass.
//!
//! A blob written before `empty_reason` existed honestly backfills to `''` per
//! case and `0` for the run: the field is absent from the document, so no case
//! in it carried one. That is the right answer, not a gap. The same holds for
//! `answered_by_provider_id`/`fallback_count`.
//!
//! # Why the passes are ordered, and where the fast paths are
//!
//! This work is synchronous, before the port is bound, and what makes that
//! acceptable is that it is bounded by *run* count — see the note in
//! `storage::cacheindex` on why the cache's equivalent had to be a background
//! task instead. A pass that decodes one blob per **case** breaks that bound,
//! so each newly-promoted column gets a fast path that derives it from columns
//! already present, and the passes run in dependency order:
//!
//! 1. [`backfill_cases`] — the migration-3/6/7/15 case columns. Only a store
//!    that predates one of those decodes here, and while it does it stamps the
//!    migration-17 answerer too, for free.
//! 2. [`backfill_runs`] — `config_digest`/`cache_hits`/`empty_count` and
//!    migration 18's `fallback_count`. Needs step 1: both of its SQL fast paths
//!    count over case rows, which have to be stamped first.
//! 3. [`backfill_case_answerers`] — migration 17's `answered_by_provider_id`
//!    for whatever step 1 did not already fill. Needs step 2: `fallback_count =
//!    0` is what licenses stamping a run's case rows `''` without reading them.
//!
//! The order cannot be rotated. Running runs before cases would make
//! `empty_count` count over unstamped `empty_reason` columns and land 0 on
//! every run of a pre-migration-15 store — silently wrong, which is worse than
//! slow.
//!
//! On the upgrade this was written for — a store whose case rows are complete
//! except for migration 17's column — step 1 selects nothing, step 2 decodes
//! one run blob per run, and step 3 stamps the overwhelming majority of case
//! rows in a single UPDATE, decoding only the cases of runs that actually fell
//! back. What the store never does is decode a blob per case.

use anyhow::Context;

use domarinn_core::result::CaseResult;

use super::decompress;
use super::exec::{params, Conn, Queryable};

/// Rows processed per `cases` chunk (one IMMEDIATE transaction each).
const CASE_CHUNK: i64 = 500;
/// Rows processed per `runs` chunk (blobs are larger; keep the chunk smaller).
const RUN_CHUNK: i64 = 100;

/// Run both backfills against the runs-database writer connection. Called from
/// `Storage::open_blocking` after migrations, so it borrows the writer
/// exclusively and does its work before any request can race it.
pub(super) fn run(conn: &mut Conn<'_>) -> anyhow::Result<()> {
    // Order is load-bearing; see the module docs.
    backfill_cases(conn)?;
    backfill_runs(conn)?;
    backfill_case_answerers(conn)?;
    // FTS index for rows that predate migration 5 (see `storage::search`).
    super::search::backfill(conn)?;
    Ok(())
}

/// The migration-3/6/7/15 case columns, from `cases.detail`.
///
/// `answered_by_provider_id` is deliberately **not** in the driving predicate,
/// even though the UPDATE below fills it: migration 17 left it NULL on every
/// historical row at once, so selecting on it would decode the whole store.
/// [`backfill_case_answerers`] owns that column and can usually avoid the
/// decode entirely. A row this pass decodes for one of the older columns gets
/// its answerer stamped on the way past, for free, and is then skipped there.
fn backfill_cases(conn: &mut Conn<'_>) -> anyhow::Result<()> {
    loop {
        // Read a chunk (materialized by `query_map`) before opening the write txn.
        let chunk: Vec<(i64, Option<Vec<u8>>)> = conn.query_map(
            "SELECT rowid, detail FROM cases
             WHERE provider_id IS NULL OR cached IS NULL OR error IS NULL
                OR empty_reason IS NULL
             LIMIT ?1",
            &params![CASE_CHUNK],
            |row| Ok((row.get::<i64>(0)?, row.get::<Option<Vec<u8>>>(1)?)),
        )?;
        if chunk.is_empty() {
            break;
        }
        let n = chunk.len();

        let mut tx = conn.immediate_tx()?;
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
                        &params![
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
                        &params![rowid],
                    )?;
                }
            }
        }
        tx.commit()?;
        tracing::debug!(cases = n, "backfill: repopulated cases chunk");
    }
    Ok(())
}

fn backfill_runs(conn: &mut Conn<'_>) -> anyhow::Result<()> {
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
        &[],
    )?;
    if fast > 0 {
        tracing::debug!(runs = fast, "backfill: counted empty cases from case rows");
    }
    // The same trick for migration 18's `fallback_count`, with the guard moved
    // to the column it counts: a run whose case rows all carry a (non-NULL)
    // answerer already holds the tally in column space, so it never needs its
    // blob. That covers every run ingested since migration 17, and every run of
    // a store old enough that `backfill_cases` above just decoded it anyway.
    //
    // It does *not* cover the migration-17/18 upgrade shape, where every case
    // row's answerer is NULL — those runs fall through to the blob loop, which
    // is the one decode this whole arrangement pays, and it is one per run
    // rather than one per case. Runs carrying an undecodable case row are
    // excluded for the same reason as above: that row's answerer is lost, so
    // counting the case rows would under-report, while the run blob may still
    // know.
    let fast = conn.execute(
        "UPDATE runs
         SET fallback_count = (
             SELECT COUNT(*) FROM cases
             WHERE cases.run_id = runs.id AND cases.answered_by_provider_id <> ''
         )
         WHERE fallback_count IS NULL
           AND NOT EXISTS (
               SELECT 1 FROM cases
               WHERE cases.run_id = runs.id
                 AND (cases.answered_by_provider_id IS NULL OR cases.cached = -1)
           )",
        &[],
    )?;
    if fast > 0 {
        tracing::debug!(
            runs = fast,
            "backfill: counted fallback answers from case rows"
        );
    }
    loop {
        let chunk: Vec<(String, Vec<u8>)> = conn.query_map(
            "SELECT run_id, body FROM run_blobs
             WHERE run_id IN (
                 SELECT id FROM runs
                 WHERE config_digest IS NULL OR cache_hits IS NULL
                    OR empty_count IS NULL OR fallback_count IS NULL
             )
             LIMIT ?1",
            &params![RUN_CHUNK],
            |row| Ok((row.get::<String>(0)?, row.get::<Vec<u8>>(1)?)),
        )?;
        if chunk.is_empty() {
            break;
        }
        let n = chunk.len();

        let mut tx = conn.immediate_tx()?;
        for (run_id, body) in chunk {
            match decode_run_columns(&body) {
                Ok(RunColumns {
                    digest,
                    cache_hits,
                    cache_misses,
                    empty_count,
                    fallback_count,
                }) => {
                    // COALESCE, not assignment: this run may be here only for
                    // `fallback_count`, with `empty_count` already established
                    // by the fast path above from its case rows. That count is
                    // the one the detail view's per-reason GROUP BY agrees
                    // with by construction; the blob's can differ. Whatever a
                    // cheaper, better-grounded pass already decided stands.
                    tx.execute(
                        "UPDATE runs
                         SET config_digest  = COALESCE(config_digest, ?1),
                             cache_hits     = COALESCE(cache_hits, ?2),
                             cache_misses   = COALESCE(cache_misses, ?3),
                             empty_count    = COALESCE(empty_count, ?4),
                             fallback_count = COALESCE(fallback_count, ?5)
                         WHERE id = ?6",
                        &params![
                            digest,
                            cache_hits,
                            cache_misses,
                            empty_count,
                            fallback_count,
                            run_id
                        ],
                    )?;
                }
                Err(error) => {
                    tracing::warn!(
                        run_id = %run_id,
                        error = %error,
                        "backfill: undecodable run blob; stamping sentinel"
                    );
                    // Sentinels only where nothing is known yet, for the same
                    // reason the success branch coalesces: a column a fast
                    // path already filled is not made worse by an unreadable
                    // blob.
                    tx.execute(
                        "UPDATE runs
                         SET config_digest  = COALESCE(config_digest, ''),
                             cache_hits     = COALESCE(cache_hits, -1),
                             cache_misses   = COALESCE(cache_misses, -1),
                             empty_count    = COALESCE(empty_count, -1),
                             fallback_count = COALESCE(fallback_count, -1)
                         WHERE id = ?1",
                        &params![run_id],
                    )?;
                }
            }
        }
        tx.commit()?;
        tracing::debug!(runs = n, "backfill: repopulated runs chunk");
    }
    Ok(())
}

/// Migration 17's `answered_by_provider_id`, for the case rows
/// [`backfill_cases`] did not already fill on its way past.
///
/// Runs on a fast path wherever migration 18's `fallback_count` can vouch for a
/// run: `= 0` means the run's document contains no answerer anywhere, so every
/// one of its case rows is stamped `''` in a single UPDATE without a byte being
/// decompressed. That is the whole store on a typical upgrade, because falling
/// back is rare.
///
/// The blanket version of that stamp — `''` everywhere, no discriminator — is
/// what this exists to avoid. `answered_by_provider_id` shipped in the client
/// before the server had a column for it, so a run uploaded in that window has
/// NULL case rows whose blobs really do name a fallback. Only `fallback_count`
/// can tell the two apart, so anything it does not vouch for (`> 0`, the -1
/// undecodable sentinel, or a run with no row at all) is decoded case by case.
fn backfill_case_answerers(conn: &mut Conn<'_>) -> anyhow::Result<()> {
    let fast = conn.execute(
        "UPDATE cases SET answered_by_provider_id = ''
         WHERE answered_by_provider_id IS NULL
           AND run_id IN (SELECT id FROM runs WHERE fallback_count = 0)",
        &[],
    )?;
    if fast > 0 {
        tracing::debug!(
            cases = fast,
            "backfill: stamped answerers for runs that never fell back"
        );
    }

    loop {
        let chunk: Vec<(i64, Option<Vec<u8>>)> = conn.query_map(
            "SELECT rowid, detail FROM cases
             WHERE answered_by_provider_id IS NULL
             LIMIT ?1",
            &params![CASE_CHUNK],
            |row| Ok((row.get::<i64>(0)?, row.get::<Option<Vec<u8>>>(1)?)),
        )?;
        if chunk.is_empty() {
            break;
        }
        let n = chunk.len();

        let mut tx = conn.immediate_tx()?;
        for (rowid, detail) in chunk {
            // `''` on both arms, and for the same tri-state reason as
            // everywhere else: a decoded case with no answerer really was
            // answered by its configured provider, and an undecodable one has
            // to stop being selected or this loop never terminates.
            let answerer = match decode_case(detail.as_deref()) {
                Ok(case) => case.answered_by_provider_id.unwrap_or_default(),
                Err(error) => {
                    tracing::warn!(
                        rowid,
                        error = %error,
                        "backfill: undecodable case detail; stamping answerer sentinel"
                    );
                    String::new()
                }
            };
            tx.execute(
                "UPDATE cases SET answered_by_provider_id = ?1 WHERE rowid = ?2",
                &params![answerer, rowid],
            )?;
        }
        tx.commit()?;
        tracing::debug!(cases = n, "backfill: repopulated answerer chunk");
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
    fallback_count: i64,
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
    let cases = value.get("cases").and_then(|v| v.as_array());
    let count_cases = |field: &str| -> i64 {
        cases
            .map(|cases| {
                cases
                    .iter()
                    .filter(|c| {
                        c.get(field)
                            .and_then(|r| r.as_str())
                            .is_some_and(|r| !r.is_empty())
                    })
                    .count() as i64
            })
            .unwrap_or(0)
    };
    // Migration 18's tally, counted the same way and over the same cases —
    // *all* of them, skipped included. It answers "can a case blob in this run
    // hold an answerer", which is exactly what `backfill_case_answerers` needs
    // and is deliberately not the client's `summary.fallback_cases` (graded
    // cases only): a run whose sole fallback answer was skipped would report 0
    // there and lose its attribution here.
    Ok(RunColumns {
        digest: digest.to_string(),
        cache_hits: count("cache_hits"),
        cache_misses: count("cache_misses"),
        empty_count: count_cases("empty_reason"),
        fallback_count: count_cases("answered_by_provider_id"),
    })
}
