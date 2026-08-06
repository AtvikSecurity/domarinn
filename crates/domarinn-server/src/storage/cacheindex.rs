//! Deriving the migration-2 columns from a cache entry's body, on write and in
//! the background for rows that predate the migration.
//!
//! The server treats `body` as opaque everywhere else, and that stays true: a
//! PUT it cannot parse is stored and served unchanged. This module is the one
//! place that *tries* to read one, and every failure it meets is recorded
//! rather than propagated.
//!
//! # Why the backfill is a background task
//!
//! [`super::backfill`] runs synchronously inside `Storage::open_blocking`,
//! before the server accepts traffic. That is right for the runs database,
//! which is bounded by run count. It would be wrong here: a shared cache can
//! hold a million entries of up to 4 MiB each, and parsing them all before
//! binding a port would turn a restart into an outage. So this drains in
//! bounded batches alongside serving, and `indexed_at IS NULL` is the progress
//! record — monotone, crash-safe, and restart-safe without a cursor to store.
//!
//! # Why there are two passes
//!
//! Migration 3 added `empty_reason`, and the rows that need it most are exactly
//! the ones migration 2 already stamped `indexed_at` — a cache poisoned by a
//! refusal months ago. `indexed_at IS NULL` cannot find them, and re-setting it
//! in the migration would be the outage this module exists to avoid. So
//! `reindexed_at IS NULL` is a second, independent progress record over the same
//! rows, drained by [`Storage::cache_reindex_batch`] after the port is bound.

use anyhow::Context;
use rusqlite::{params, Connection, TransactionBehavior};
use serde_json::Value as Json;

use domarinn_core::cache::CacheEntry;
use domarinn_core::types::Output;

use super::{now_ms, to_microusd, Storage};

/// Characters of request/output text handed to FTS per entry.
///
/// A cap rather than the whole body: an entry may be 4 MiB, the index would be
/// the same size again, and the hundredth kilobyte of a response is not what
/// anyone searches for.
const FTS_TEXT_MAX: usize = 16 * 1024;
/// Characters of `request_summary`. It is one line in a table.
const SUMMARY_MAX: usize = 256;
/// Characters of `output_preview`, mirroring `cases.output_preview`.
const PREVIEW_MAX: usize = 300;
/// Everything migrations 2 and 3 promote, derived from one body.
pub(crate) struct EntryIndex {
    pub kind: Option<String>,
    pub model: Option<String>,
    pub cost_microusd: Option<i64>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub entry_created_at: Option<i64>,
    pub request_summary: Option<String>,
    pub output_preview: Option<String>,
    /// Why the output was empty, clamped. `None` means the entry recorded no
    /// reason — a real answer, or a build that predates the field.
    pub empty_reason: Option<String>,
    pub fts_request: String,
    pub fts_output: String,
}

impl EntryIndex {
    /// Derive the promoted columns, or `None` when the body is not an entry
    /// this build understands.
    ///
    /// `None` is not an error. It is the ordinary answer for a blob written by
    /// a newer domarinn, or by something that is not domarinn at all, and the
    /// caller records it as "looked at, found nothing" so the row is never
    /// examined again.
    pub(crate) fn derive(body: &[u8]) -> Option<EntryIndex> {
        let entry: CacheEntry = serde_json::from_slice(body).ok()?;
        Some(EntryIndex::from_entry(&entry))
    }

    /// The same projection, from an entry that is already parsed.
    ///
    /// Split out so the local disk tier — which reads through a backend that
    /// hands back a `CacheEntry` rather than bytes — describes an entry the
    /// same way the server tier does. Two definitions of "what a row shows"
    /// would drift, and the drift would look like the tiers disagreeing about
    /// the same cache.
    pub(crate) fn from_entry(entry: &CacheEntry) -> EntryIndex {
        let output_text = output_to_text(&entry.output);
        let request_text = entry
            .request
            .as_ref()
            .map(|r| r.to_string())
            .unwrap_or_default();

        EntryIndex {
            kind: entry.inferred_kind(),
            model: entry.model.clone(),
            cost_microusd: to_microusd(entry.cost_usd),
            input_tokens: entry.usage.as_ref().map(|u| u.input_tokens as i64),
            output_tokens: entry.usage.as_ref().map(|u| u.output_tokens as i64),
            entry_created_at: Some(entry.created_at.timestamp_millis()),
            request_summary: request_summary(entry.request.as_ref()),
            output_preview: Some(truncate(&output_text, PREVIEW_MAX)),
            empty_reason: entry.empty_reason.as_ref().map(clamp_empty_reason),
            // `empty_reason` is deliberately absent from both FTS columns. It
            // is a facet and a filter, and folding it into free text would make
            // `q=refusal` match every refused entry *and* every entry whose
            // output happens to discuss one, with no way to tell them apart.
            fts_request: truncate(&request_text, FTS_TEXT_MAX),
            fts_output: truncate(&output_text, FTS_TEXT_MAX),
        }
    }
}

/// Make an entry's `empty_reason` safe to store, log and facet on.
///
/// `EmptyReason` is an open string newtype by design (`domarinn_types::empty`),
/// and an `exec` provider child sets it verbatim from its own stdout — domarinn
/// does not control that writer in any version. Two consequences follow, and
/// neither is hypothetical:
///
/// * A reason containing a newline lands in a `tracing` field and forges log
///   lines; one containing a control character corrupts a terminal reading them.
/// * An unbounded reason is an unbounded *facet* value, and the reason facet is
///   uncapped — a child emitting a fresh 4 KiB reason per call would make
///   `/cache/facets` grow without limit.
///
/// So: control characters out, length capped. The stored entry body keeps the
/// original verbatim; this is only what the index promotes.
pub(crate) fn clamp_empty_reason(reason: &domarinn_core::empty::EmptyReason) -> String {
    // Delegated rather than reimplemented: the disk tier's purge filter compares
    // on the same form, and two copies of this rule is how one tier starts
    // evicting entries the other cannot find.
    reason.clamped()
}

/// A one-line description of where the request went.
///
/// Deliberately address-only. This is the field rendered in a list view, and a
/// summary built from prompt text would make the list itself a corpus, one
/// scroll at a time — the body is one click away in the drawer, which is the
/// natural boundary for looking at content.
fn request_summary(request: Option<&Json>) -> Option<String> {
    let request = request?;
    let summary = match request.get("transport").and_then(Json::as_str) {
        Some("http") => {
            let method = request
                .get("method")
                .and_then(Json::as_str)
                .unwrap_or("POST");
            let url = request.get("url").and_then(Json::as_str)?;
            format!("{method} {}", strip_query(url))
        }
        Some("exec") => {
            let command = request.get("command").and_then(Json::as_str)?;
            let args = request
                .get("args")
                .and_then(Json::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Json::as_str)
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .unwrap_or_default();
            if args.is_empty() {
                format!("exec {command}")
            } else {
                format!("exec {command} {args}")
            }
        }
        _ => return None,
    };
    Some(truncate(&summary, SUMMARY_MAX))
}

/// Drop everything from the first `?` or `#`.
///
/// A canonical request has no headers — credentials are structurally absent
/// from both envelope shapes — but a self-hosted gateway can carry a key in its
/// query string, and the summary must not be where that surfaces.
fn strip_query(url: &str) -> &str {
    let end = url.find(['?', '#']).unwrap_or(url.len());
    &url[..end]
}

fn output_to_text(output: &Output) -> String {
    match output {
        Output::Text(text) => text.clone(),
        Output::Json(value) => value.to_string(),
    }
}

/// Truncate on a character boundary, so a multi-byte codepoint is never split.
fn truncate(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((byte, _)) => text[..byte].to_string(),
        None => text.to_string(),
    }
}

impl Storage {
    /// Index up to `batch` rows that have never been looked at.
    ///
    /// Returns how many were stamped. `0` means the backfill has drained.
    ///
    /// Two phases, and the split is the point: parsing happens on a **pooled
    /// reader**, and only SQL runs under the writer. `Db::write` acquires the
    /// single writer mutex and *then* runs its closure, so parsing a batch of
    /// bodies inside one would stall every concurrent `cache_get` and
    /// `cache_put` for the duration.
    pub async fn cache_index_batch(&self, batch: i64) -> anyhow::Result<usize> {
        let derived: Vec<(i64, Option<EntryIndex>)> =
            self.cache.read(move |conn| read_batch(conn, batch)).await?;
        if derived.is_empty() {
            return Ok(0);
        }
        self.cache
            .write(move |conn| write_batch(conn, derived))
            .await
    }

    /// Re-derive up to `batch` rows that were indexed by a build older than
    /// migration 3, so their `empty_reason` stops being permanently unknown.
    ///
    /// Returns how many were stamped. `0` means the pass has drained, and it
    /// drains for good: every row it visits gets `reindexed_at`, including the
    /// unparseable ones it declines to re-read.
    ///
    /// Same two-phase split as [`Storage::cache_index_batch`], for the same
    /// reason — parsing happens on a pooled reader, only SQL runs under the
    /// single writer.
    pub async fn cache_reindex_batch(&self, batch: i64) -> anyhow::Result<usize> {
        let derived: Vec<Reindexed> = self
            .cache
            .read(move |conn| read_reindex_batch(conn, batch))
            .await?;
        if derived.is_empty() {
            return Ok(0);
        }
        self.cache
            .write(move |conn| write_reindex_batch(conn, derived))
            .await
    }

    /// What each backfill pass still has to look at.
    ///
    /// One query per pass rather than a `FILTER` over a single aggregate, so
    /// each is served straight from its own partial index — both of which are
    /// empty in the steady state, which is where a running server spends its
    /// life.
    pub async fn cache_backfill_remaining(&self) -> anyhow::Result<BackfillRemaining> {
        self.cache
            .read(|conn| {
                Ok(BackfillRemaining {
                    unindexed: conn.query_row(
                        "SELECT COUNT(*) FROM cache_entries WHERE indexed_at IS NULL",
                        [],
                        |r| r.get(0),
                    )?,
                    stale: conn.query_row(
                        "SELECT COUNT(*) FROM cache_entries
                          WHERE reindexed_at IS NULL AND indexed_at IS NOT NULL",
                        [],
                        |r| r.get(0),
                    )?,
                })
            })
            .await
    }
}

/// Rows still owed to each pass. Reported so the backfill has *some* observable
/// progress: before this it logged a per-batch count at `debug!` and nothing
/// else, so "is it stuck or is it working" had no answer from outside.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BackfillRemaining {
    pub unindexed: i64,
    pub stale: i64,
}

impl BackfillRemaining {
    pub fn total(self) -> i64 {
        self.unindexed + self.stale
    }
}

/// One row the re-index pass looked at, and what it found.
///
/// `None` carries a decision, not a failure: the row is stamped without its
/// body ever being read, because it is already recorded as unparseable.
struct Reindexed {
    rowid: i64,
    index: Option<EntryIndex>,
}

fn read_reindex_batch(conn: &Connection, batch: i64) -> anyhow::Result<Vec<Reindexed>> {
    // `indexed_at IS NOT NULL` keeps the two passes disjoint. A row that has
    // never been looked at is in both partial indexes, and letting this pass
    // claim it would derive it twice and stamp only half the state.
    let candidates: Vec<(i64, Option<i64>)> = {
        let mut stmt = conn.prepare(
            "SELECT rowid, index_ok FROM cache_entries
              WHERE reindexed_at IS NULL AND indexed_at IS NOT NULL
              ORDER BY rowid LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![batch], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect::<Result<Vec<_>, _>>()?
    };

    let mut out = Vec::with_capacity(candidates.len());
    let mut budget = READ_BYTE_BUDGET;
    for (rowid, index_ok) in candidates {
        // Already known not to parse. Stamping it without a read is what makes
        // this pass terminate rather than re-failing on the same rows forever.
        if index_ok != Some(1) {
            out.push(Reindexed { rowid, index: None });
            continue;
        }
        let body: Option<Vec<u8>> = conn
            .query_row(
                "SELECT body FROM cache_entries WHERE rowid = ?1",
                params![rowid],
                |row| row.get(0),
            )
            .ok();
        // Gone since the id was read — a prune ran between the two statements.
        let Some(body) = body else { continue };
        budget = budget.saturating_sub(body.len());
        out.push(Reindexed {
            rowid,
            index: EntryIndex::derive(&body),
        });
        if budget == 0 {
            break;
        }
    }
    Ok(out)
}

fn write_reindex_batch(conn: &mut Connection, derived: Vec<Reindexed>) -> anyhow::Result<usize> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let now = now_ms();
    let mut stamped = 0usize;
    for Reindexed { rowid, index } in derived {
        let Some(index) = index else {
            // Nothing was read, so nothing is re-derived; the stamp alone takes
            // the row out of the pending index. `index_ok` keeps its verdict.
            stamped += tx.execute(
                "UPDATE cache_entries SET reindexed_at = ?1
                  WHERE rowid = ?2 AND reindexed_at IS NULL",
                params![now, rowid],
            )?;
            continue;
        };
        // `AND reindexed_at IS NULL` closes the same read-then-write race
        // `write_batch` documents: a prune may have deleted this row and
        // SQLite may have handed its rowid to a new entry.
        let changed = tx.execute(
            "UPDATE cache_entries
                SET reindexed_at = ?1, kind = ?2, model = ?3,
                    cost_microusd = ?4, input_tokens = ?5, output_tokens = ?6,
                    entry_created_at = ?7, request_summary = ?8, output_preview = ?9,
                    empty_reason = ?10
              WHERE rowid = ?11 AND reindexed_at IS NULL",
            params![
                now,
                index.kind,
                index.model,
                index.cost_microusd,
                index.input_tokens,
                index.output_tokens,
                index.entry_created_at,
                index.request_summary,
                index.output_preview,
                index.empty_reason,
                rowid,
            ],
        )?;
        if changed == 0 {
            continue;
        }
        stamped += 1;
        insert_fts(&tx, rowid, &index)?;
    }
    tx.commit()?;
    Ok(stamped)
}

/// Bytes of body held in memory before a batch cuts itself short.
///
/// Rows are fetched one body at a time rather than in one query, so peak
/// memory is one body plus the batch's small derived structs — not `batch`
/// times the 4 MiB entry cap.
const READ_BYTE_BUDGET: usize = 16 * 1024 * 1024;

fn read_batch(conn: &Connection, batch: i64) -> anyhow::Result<Vec<(i64, Option<EntryIndex>)>> {
    let rowids: Vec<i64> = {
        let mut stmt = conn.prepare(
            "SELECT rowid FROM cache_entries WHERE indexed_at IS NULL ORDER BY rowid LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![batch], |row| row.get::<_, i64>(0))?;
        rows.collect::<Result<Vec<i64>, _>>()?
    };

    let mut out = Vec::with_capacity(rowids.len());
    let mut budget = READ_BYTE_BUDGET;
    for rowid in rowids {
        let body: Option<Vec<u8>> = conn
            .query_row(
                "SELECT body FROM cache_entries WHERE rowid = ?1",
                params![rowid],
                |row| row.get(0),
            )
            .ok();
        // Gone since the id was read — a prune ran between the two statements.
        let Some(body) = body else { continue };
        budget = budget.saturating_sub(body.len());
        out.push((rowid, EntryIndex::derive(&body)));
        if budget == 0 {
            break;
        }
    }
    Ok(out)
}

fn write_batch(
    conn: &mut Connection,
    derived: Vec<(i64, Option<EntryIndex>)>,
) -> anyhow::Result<usize> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let now = now_ms();
    let mut stamped = 0usize;
    for (rowid, index) in derived {
        // `AND indexed_at IS NULL` closes the read-then-write race: a prune may
        // have deleted this row and SQLite may have handed its rowid to a new
        // entry, which would otherwise be stamped with another entry's
        // metadata. Not decorative.
        let changed = tx.execute(
            "UPDATE cache_entries
                SET indexed_at = ?1, reindexed_at = ?1, index_ok = ?2, kind = ?3, model = ?4,
                    cost_microusd = ?5, input_tokens = ?6, output_tokens = ?7,
                    entry_created_at = ?8, request_summary = ?9, output_preview = ?10,
                    empty_reason = ?11
              WHERE rowid = ?12 AND indexed_at IS NULL",
            params![
                now,
                index.is_some() as i64,
                index.as_ref().and_then(|i| i.kind.clone()),
                index.as_ref().and_then(|i| i.model.clone()),
                index.as_ref().and_then(|i| i.cost_microusd),
                index.as_ref().and_then(|i| i.input_tokens),
                index.as_ref().and_then(|i| i.output_tokens),
                index.as_ref().and_then(|i| i.entry_created_at),
                index.as_ref().and_then(|i| i.request_summary.clone()),
                index.as_ref().and_then(|i| i.output_preview.clone()),
                index.as_ref().and_then(|i| i.empty_reason.clone()),
                rowid,
            ],
        )?;
        if changed == 0 {
            continue;
        }
        stamped += 1;
        if let Some(index) = index {
            insert_fts(&tx, rowid, &index)?;
        }
    }
    tx.commit()?;
    Ok(stamped)
}

/// Replace one entry's searchable text, rowid-aligned with `cache_entries` so
/// the delete trigger can find it without scanning.
///
/// # Why the DELETE is not redundant
///
/// `cache_entries_fts` is a plain (content-full) fts5 table, and fts5 backs
/// those with a shadow `%_content(id INTEGER PRIMARY KEY)`. This INSERT
/// supplies an **explicit** rowid, so a second insert for a rowid that already
/// has a row raises `SQLITE_CONSTRAINT` — it does *not* silently duplicate.
///
/// That matters because the re-index pass visits rows that were indexed by an
/// older build, and every one of those already has an fts row. Without the
/// DELETE, the first such row would fail, the `?` would propagate out of
/// [`write_reindex_batch`] before its `commit`, and the whole batch would roll
/// back **including its `reindexed_at` stamps**. The driver would warn, sleep,
/// re-read the identical batch, and fail identically — forever, never reaching
/// the rows the pass exists to fix.
pub(super) fn insert_fts(conn: &Connection, rowid: i64, index: &EntryIndex) -> anyhow::Result<()> {
    conn.execute(
        "DELETE FROM cache_entries_fts WHERE rowid = ?1",
        params![rowid],
    )
    .context("clearing stale cache entry text")?;
    conn.execute(
        "INSERT INTO cache_entries_fts (rowid, request, output) VALUES (?1, ?2, ?3)",
        params![rowid, index.fts_request, index.fts_output],
    )
    .context("indexing cache entry text")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_query_string_never_reaches_a_summary() {
        assert_eq!(
            strip_query("https://host/v1/messages?api_key=secret"),
            "https://host/v1/messages"
        );
        assert_eq!(strip_query("https://host/v1#frag"), "https://host/v1");
        assert_eq!(strip_query("https://host/v1"), "https://host/v1");
    }

    /// The same argument as [`a_query_string_never_reaches_a_summary`], for a
    /// field a provider child writes verbatim: an `exec` grader can put
    /// anything at all in `empty_reason`, and it reaches a `tracing` field and
    /// an uncapped facet. Newlines forge log lines; length is facet cardinality.
    #[test]
    fn a_reason_an_exec_child_invented_cannot_forge_a_log_line() {
        use domarinn_core::empty::EmptyReason;

        assert_eq!(
            clamp_empty_reason(&EmptyReason::new("refusal\n2026-01-01 ERROR forged")),
            "refusal2026-01-01 ERROR forged"
        );
        assert_eq!(
            clamp_empty_reason(&EmptyReason::new("a\r\nb\tc\u{7}")),
            "abc"
        );
        assert_eq!(
            clamp_empty_reason(&EmptyReason::new("x".repeat(4096))).len(),
            EmptyReason::CLAMP_MAX
        );
        // An ordinary reason is untouched, including one this build has never
        // heard of — the type is open by construction.
        assert_eq!(
            clamp_empty_reason(&EmptyReason::new("invented_next_year")),
            "invented_next_year"
        );
    }

    /// Truncation is by character, not byte: slicing a multi-byte codepoint in
    /// half would panic on a response in any non-ASCII language.
    #[test]
    fn truncation_lands_on_a_character_boundary() {
        let text = "héllo wörld";
        assert_eq!(truncate(text, 4), "héll");
        assert_eq!(truncate(text, 100), text);
        assert_eq!(truncate("日本語です", 2), "日本");
    }
}
