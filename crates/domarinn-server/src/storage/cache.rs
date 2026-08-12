//! Content-addressed cache table operations: get (with hit/miss accounting),
//! has, immutable put, stats, single-entry delete, and filtered/size pruning
//! (also used by the retention task).

use anyhow::Context;

use super::cacheindex::{self, EntryIndex};
use super::exec::{params, Conn, Queryable, Value};
use super::{ms_to_rfc3339, now_ms, CachePutOutcome, Storage};
use crate::dto::cache::CacheStatsResponse;

impl Storage {
    pub async fn cache_get(&self, key: String) -> anyhow::Result<Option<Vec<u8>>> {
        self.cache
            .write(move |conn| {
                let mut tx = conn.immediate_tx()?;
                let body: Option<Vec<u8>> = tx.query_row_opt(
                    "SELECT body FROM cache_entries WHERE key = ?1",
                    &params![key],
                    |row| row.get(0),
                )?;
                match &body {
                    Some(_) => {
                        tx.execute(
                            "UPDATE cache_entries SET last_access_at = ?2 WHERE key = ?1",
                            &params![key, now_ms()],
                        )?;
                        tx.execute(
                            "UPDATE cache_counters SET hits = hits + 1 WHERE id = 1",
                            &[],
                        )?;
                    }
                    None => {
                        tx.execute(
                            "UPDATE cache_counters SET misses = misses + 1 WHERE id = 1",
                            &[],
                        )?;
                    }
                }
                tx.commit()?;
                Ok(body)
            })
            .await
    }

    /// Existence probe behind `HEAD`. Deliberately does *not* touch the
    /// hits/misses counters: the domarinn client only ever `GET`s, so counting
    /// probes would inflate the hit rate the server reports. A found entry
    /// still refreshes `last_access_at` so a probed entry is not evicted next.
    pub async fn cache_has(&self, key: String) -> anyhow::Result<bool> {
        self.cache
            .write(move |conn| {
                let mut tx = conn.immediate_tx()?;
                let found = tx
                    .query_row_opt(
                        "SELECT 1 FROM cache_entries WHERE key = ?1",
                        &params![key],
                        |_| Ok(()),
                    )?
                    .is_some();
                if found {
                    tx.execute(
                        "UPDATE cache_entries SET last_access_at = ?2 WHERE key = ?1",
                        &params![key, now_ms()],
                    )?;
                }
                tx.commit()?;
                Ok(found)
            })
            .await
    }

    /// Store an entry, deriving its browsable columns on the way in.
    ///
    /// A body this server cannot parse is stored anyway, stamped as looked-at
    /// with every promoted column NULL. That is not leniency, it is the
    /// contract: the cache is a blob store, the client owns the format, and a
    /// PUT that a *newer* client understands must not be rejected by an older
    /// server that does not.
    pub async fn cache_put(&self, key: String, body: Vec<u8>) -> anyhow::Result<CachePutOutcome> {
        // Derive before taking the writer. `Db::write` locks the single writer
        // mutex and then runs its closure, so parsing a 4 MiB body inside one
        // would stall every concurrent get and put for the duration.
        let (body, index) = tokio::task::spawn_blocking(move || {
            let index = EntryIndex::derive(&body);
            (body, index)
        })
        .await
        .context("cache index task panicked")?;

        self.cache
            .write(move |conn| {
                let mut tx = conn.immediate_tx()?;
                let now = now_ms();
                let size = body.len() as i64;
                // First-write-wins: `ON CONFLICT DO NOTHING RETURNING` answers
                // "did we win" and hands back the row's id for the FTS insert
                // in one statement (`last_insert_rowid()` has no Postgres
                // analogue). `reindexed_at` is stamped here with `indexed_at`:
                // this row was just derived by *this* build, so the
                // migration-3 catch-up pass has nothing to do with it. Leaving
                // it NULL would put every new write into that pass's queue and
                // re-read its body once more, for a column it already holds.
                let sql = format!(
                    "INSERT INTO cache_entries
                         (key, body, size, created_at, last_access_at,
                          indexed_at, reindexed_at, index_ok, kind, model, cost_microusd,
                          input_tokens, output_tokens, entry_created_at,
                          request_summary, output_preview, empty_reason)
                     VALUES (?1, ?2, ?3, ?4, ?4, ?5, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
                     ON CONFLICT (key) DO NOTHING
                     RETURNING {id}",
                    id = cacheindex::entry_id_col(tx.dialect()),
                );
                let won: Option<i64> = tx.query_row_opt(
                    &sql,
                    &params![
                        key,
                        body,
                        size,
                        now,
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
                    ],
                    |row| row.get(0),
                )?;
                let outcome = if let Some(rowid) = won {
                    // Only the winner indexes: an entry is immutable, so a
                    // losing PUT has nothing to add and a second FTS row would
                    // make the same entry match twice.
                    if let Some(index) = &index {
                        cacheindex::insert_fts(&mut tx, rowid, index)?;
                    }
                    CachePutOutcome::Created
                } else {
                    CachePutOutcome::Exists
                };
                tx.commit()?;
                Ok(outcome)
            })
            .await
    }

    pub async fn cache_stats(&self) -> anyhow::Result<CacheStatsResponse> {
        self.cache.read(cache_stats).await
    }

    /// Delete one entry by key. `false` means there was nothing there.
    ///
    /// The `cache_entries_delete_fts` trigger keeps the search index in step,
    /// so this is a single statement rather than the explicit two-table delete
    /// `storage::retention` needs on the runs side.
    pub async fn cache_delete_entry(&self, key: String) -> anyhow::Result<bool> {
        self.cache
            .write(move |conn| {
                let removed =
                    conn.execute("DELETE FROM cache_entries WHERE key = ?1", &params![key])?;
                Ok(removed > 0)
            })
            .await
    }

    /// Prune by predicate and/or a total-size target (LRU eviction to reach it).
    #[tracing::instrument(skip_all, fields(filter = ?filter))]
    pub async fn cache_prune(&self, filter: CachePruneFilter) -> anyhow::Result<u64> {
        self.cache
            .write(move |conn| cache_prune(conn, &filter))
            .await
    }
}

/// What a prune is allowed to delete.
///
/// A struct rather than positional arguments, because the predicate set has
/// grown past the point where `cache_prune(None, Some(x))` reads as anything —
/// and because [`Default`] now has to mean something specific: a filter that
/// names nothing deletes nothing. See [`cache_prune`].
#[derive(Debug, Clone, Default)]
pub struct CachePruneFilter {
    /// Entries first stored more than this many days ago.
    pub older_than_days: Option<i64>,
    /// Entries first stored *within* this many days. Paired with
    /// `older_than_days` it names a window; alone it names everything recent,
    /// which is why the route requires a caller to ask for it explicitly.
    pub newer_than_days: Option<i64>,
    /// Entries whose promoted `empty_reason` is one of these. An empty vector
    /// is "no such predicate", never "entries with no reason" — the difference
    /// is the whole reason [`Self::is_empty`] exists.
    pub empty_reason: Vec<String>,
    pub model: Option<String>,
    pub kind: Option<String>,
    /// LRU-evict until the store is under this many bytes. Independent of
    /// every predicate above: it describes the store's size, not a row.
    pub target_bytes: Option<i64>,
}

impl CachePruneFilter {
    /// Whether this filter names nothing at all — no predicate and no size
    /// target. The retention route's "a bare prune means the configured
    /// limits" rule keys off exactly this, and off nothing narrower: a prune
    /// that named `empty_reason` must apply *only* that, or asking to evict
    /// refusals would also silently apply `max_age_days` and `max_bytes`.
    pub fn is_empty(&self) -> bool {
        self.older_than_days.is_none()
            && self.newer_than_days.is_none()
            && self.empty_reason.is_empty()
            && self.model.is_none()
            && self.kind.is_none()
            && self.target_bytes.is_none()
    }
}

fn cache_stats(conn: &mut Conn<'_>) -> anyhow::Result<CacheStatsResponse> {
    let (entries, total_bytes, oldest): (i64, i64, Option<i64>) = conn.query_row(
        "SELECT COUNT(*), CAST(COALESCE(SUM(size), 0) AS BIGINT), MIN(created_at) FROM cache_entries",
        &[],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    let (hits, misses): (i64, i64) = conn.query_row(
        "SELECT hits, misses FROM cache_counters WHERE id = 1",
        &[],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    // Its own query rather than a FILTER on the aggregate above, so the partial
    // index serves it directly — and so it costs nothing once the backfill has
    // drained, which is the steady state.
    let unindexed: i64 = conn.query_row(
        "SELECT COUNT(*) FROM cache_entries WHERE indexed_at IS NULL",
        &[],
        |row| row.get(0),
    )?;
    Ok(CacheStatsResponse {
        entries,
        total_bytes,
        hits,
        misses,
        unindexed,
        oldest_entry_at: oldest.map(ms_to_rfc3339),
    })
}

/// `now - days`, saturating.
///
/// `retention::sweep`'s lesson, applied to a value that reaches SQL from a query
/// string: `days * 86_400_000` wraps for a large `i64` and release builds wrap
/// silently, putting the cutoff in the *future* so an "older than" prune deletes
/// the entire store. The route additionally rejects negative days, but the
/// arithmetic must not be the only thing standing between a typo and the cache.
fn cutoff_ms(days: i64) -> i64 {
    let window = (days as i128) * 86_400_000i128;
    (now_ms() as i128 - window).clamp(i64::MIN as i128, i64::MAX as i128) as i64
}

/// Turn a filter into `WHERE` clauses and their bound values.
///
/// Returns an **empty** clause list for a filter that names no predicate. That
/// is the load-bearing case: see [`cache_prune`].
fn prune_predicate(filter: &CachePruneFilter) -> (Vec<String>, Vec<Value>) {
    let mut clauses: Vec<String> = Vec::new();
    let mut args: Vec<Value> = Vec::new();

    if let Some(days) = filter.older_than_days {
        args.push(cutoff_ms(days).into());
        clauses.push(format!("created_at < ?{}", args.len()));
    }
    if let Some(days) = filter.newer_than_days {
        args.push(cutoff_ms(days).into());
        clauses.push(format!("created_at >= ?{}", args.len()));
    }
    if !filter.empty_reason.is_empty() {
        let placeholders: Vec<String> = filter
            .empty_reason
            .iter()
            .map(|reason| {
                args.push(reason.clone().into());
                format!("?{}", args.len())
            })
            .collect();
        // `IN` over a NOT NULL column, so an entry that recorded no reason is
        // never matched — evicting "everything that is not a refusal" is not
        // something any caller here can ask for by accident.
        clauses.push(format!("empty_reason IN ({})", placeholders.join(", ")));
    }
    if let Some(model) = &filter.model {
        args.push(model.clone().into());
        clauses.push(format!("model = ?{}", args.len()));
    }
    if let Some(kind) = &filter.kind {
        args.push(kind.clone().into());
        clauses.push(format!("kind = ?{}", args.len()));
    }
    (clauses, args)
}

/// Delete what the filter names, then LRU-evict to its size target.
///
/// # Never `WHERE 1=1` in a DELETE
///
/// The list query in `storage::cachebrowse` opens with `WHERE 1=1` and appends
/// ` AND …` per filter, which is a fine idiom for a SELECT and a catastrophic
/// one here: transplanted to a DELETE, a filter that named nothing would match
/// every row and wipe a cache several teams share. Two callers can present such
/// a filter, and one of them — the hourly retention task — is unattended. So the
/// clauses are collected into a `Vec` and an empty one **skips the statement
/// entirely** rather than falling through to a bare `DELETE FROM`.
fn cache_prune(conn: &mut Conn<'_>, filter: &CachePruneFilter) -> anyhow::Result<u64> {
    let mut tx = conn.immediate_tx()?;
    let mut pruned = 0u64;

    let (clauses, args) = prune_predicate(filter);
    if !clauses.is_empty() {
        let sql = format!("DELETE FROM cache_entries WHERE {}", clauses.join(" AND "));
        pruned += tx.execute(&sql, &args)?;
    }

    if let Some(target) = filter.target_bytes {
        let mut total: i64 = tx.query_row(
            "SELECT CAST(COALESCE(SUM(size), 0) AS BIGINT) FROM cache_entries",
            &[],
            |row| row.get(0),
        )?;
        if total > target {
            // Evict least-recently-accessed first until under target.
            let victims: Vec<(String, i64)> = tx.query_map(
                "SELECT key, size FROM cache_entries ORDER BY last_access_at ASC",
                &[],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            for (key, size) in victims {
                if total <= target {
                    break;
                }
                tx.execute("DELETE FROM cache_entries WHERE key = ?1", &params![key])?;
                total -= size;
                pruned += 1;
            }
        }
    }

    tx.commit()?;
    Ok(pruned)
}
