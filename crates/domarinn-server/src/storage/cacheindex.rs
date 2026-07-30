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

use anyhow::Context;
use rusqlite::{params, Connection, TransactionBehavior};
use serde_json::Value as Json;

use domarinn_core::cache::{CacheEntry, EntryKind, GradedVerdict};
use domarinn_core::types::Output;

use super::{now_ms, to_microusd, Storage};

/// A `similar` assertion's ≤0.4.x cosine verdict.
///
/// Not [`EntryKind::EMBEDDING`]: an embedding entry holds one vector, where
/// this holds the comparison of two. They are different things that happen to
/// come from the same assertion, and collapsing them would make a `kind` filter
/// return entries that cannot answer the same question.
pub(crate) const SIMILAR_VERDICT: &str = "similar_verdict";

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

/// Everything migration 2 promotes, derived from one body.
pub(crate) struct EntryIndex {
    pub kind: Option<String>,
    pub model: Option<String>,
    pub cost_microusd: Option<i64>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub entry_created_at: Option<i64>,
    pub request_summary: Option<String>,
    pub output_preview: Option<String>,
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
            kind: infer_kind(entry),
            model: entry.model.clone(),
            cost_microusd: to_microusd(entry.cost_usd),
            input_tokens: entry.usage.as_ref().map(|u| u.input_tokens as i64),
            output_tokens: entry.usage.as_ref().map(|u| u.output_tokens as i64),
            entry_created_at: Some(entry.created_at.timestamp_millis()),
            request_summary: request_summary(entry.request.as_ref()),
            output_preview: Some(truncate(&output_text, PREVIEW_MAX)),
            fts_request: truncate(&request_text, FTS_TEXT_MAX),
            fts_output: truncate(&output_text, FTS_TEXT_MAX),
        }
    }
}

/// What kind of call an entry answers, first match wins.
///
/// The rungs are ordered by how much they are *claiming*. An explicit kind is a
/// statement the writer made; everything below it is a reader's guess, and a
/// guess must never overrule a statement — including a kind string this build
/// has never heard of, which is exactly the case an open
/// [`EntryKind`] exists to carry.
fn infer_kind(entry: &CacheEntry) -> Option<String> {
    // 1. The writer said so.
    if let Some(kind) = &entry.kind {
        return Some(kind.as_str().to_string());
    }

    // 2. A ≤0.4.x verdict entry self-identifies: `GradedVerdict` is
    //    `#[serde(tag = "kind")]`, so the stored shape names its own grader.
    if let Some(verdict) = &entry.verdict {
        return Some(
            match verdict {
                GradedVerdict::Rubric { .. } => EntryKind::JUDGE,
                GradedVerdict::Exec { .. } => EntryKind::EXEC_ASSERT,
                GradedVerdict::Similarity { .. } => SIMILAR_VERDICT,
            }
            .to_string(),
        );
    }

    // 3. An `exec` request carries the protocol envelope it wrote to the
    //    child's stdin, and that envelope names the call kind.
    if let Some(request) = &entry.request {
        if request.get("transport").and_then(Json::as_str) == Some("exec") {
            match request
                .pointer("/stdin/domarinn/kind")
                .and_then(Json::as_str)
            {
                Some("provider") => return Some(EntryKind::PROVIDER.to_string()),
                Some("assert") => return Some(EntryKind::EXEC_ASSERT.to_string()),
                _ => {}
            }
        }
    }

    // 4. An embedding's output is a dimension count and nothing else — the
    //    grader writes that shape deliberately so an entry is legible.
    if let Output::Json(value) = &entry.output {
        if let Some(map) = value.as_object() {
            if map.len() == 1 && map.contains_key("dims") {
                return Some(EntryKind::EMBEDDING.to_string());
            }
        }
    }

    // 5. Only the provider path goes through `with_retry`, so only it has an
    //    attempt count or an in-flight measurement to record. The grader path
    //    sets both to `None` and says why.
    if entry.attempts.is_some() || entry.provider_latency_ms.is_some() {
        return Some(EntryKind::PROVIDER.to_string());
    }

    // 6. Nothing distinguishes an old HTTP judge call from an old HTTP provider
    //    call. No kind is the honest answer; a default would be a fabrication
    //    that a filter would then treat as fact.
    None
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
                SET indexed_at = ?1, index_ok = ?2, kind = ?3, model = ?4,
                    cost_microusd = ?5, input_tokens = ?6, output_tokens = ?7,
                    entry_created_at = ?8, request_summary = ?9, output_preview = ?10
              WHERE rowid = ?11 AND indexed_at IS NULL",
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

/// Add one entry's searchable text, rowid-aligned with `cache_entries` so the
/// delete trigger can find it without scanning.
pub(super) fn insert_fts(conn: &Connection, rowid: i64, index: &EntryIndex) -> anyhow::Result<()> {
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
