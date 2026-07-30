//! Content-addressed cache table operations: get (with hit/miss accounting),
//! has, immutable put, stats, and age/size pruning (also used by the retention
//! task).

use anyhow::Context;
use rusqlite::{params, Connection, TransactionBehavior};

use super::cacheindex::{self, EntryIndex};
use super::{ms_to_rfc3339, now_ms, CachePutOutcome, Storage};
use crate::dto::cache::CacheStatsResponse;

impl Storage {
    pub async fn cache_get(&self, key: String) -> anyhow::Result<Option<Vec<u8>>> {
        self.cache
            .write(move |conn| {
                let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
                let body: Option<Vec<u8>> = tx
                    .query_row(
                        "SELECT body FROM cache_entries WHERE key = ?1",
                        params![key],
                        |row| row.get(0),
                    )
                    .ok();
                match &body {
                    Some(_) => {
                        tx.execute(
                            "UPDATE cache_entries SET last_access_at = ?2 WHERE key = ?1",
                            params![key, now_ms()],
                        )?;
                        tx.execute("UPDATE cache_counters SET hits = hits + 1 WHERE id = 1", [])?;
                    }
                    None => {
                        tx.execute(
                            "UPDATE cache_counters SET misses = misses + 1 WHERE id = 1",
                            [],
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
                let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
                let found = tx
                    .query_row(
                        "SELECT 1 FROM cache_entries WHERE key = ?1",
                        params![key],
                        |_| Ok(()),
                    )
                    .is_ok();
                if found {
                    tx.execute(
                        "UPDATE cache_entries SET last_access_at = ?2 WHERE key = ?1",
                        params![key, now_ms()],
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
                let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
                let now = now_ms();
                let size = body.len() as i64;
                // First-write-wins: INSERT OR IGNORE, then detect whether we won.
                let n = tx.execute(
                    "INSERT OR IGNORE INTO cache_entries
                         (key, body, size, created_at, last_access_at,
                          indexed_at, index_ok, kind, model, cost_microusd,
                          input_tokens, output_tokens, entry_created_at,
                          request_summary, output_preview)
                     VALUES (?1, ?2, ?3, ?4, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                    params![
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
                    ],
                )?;
                let outcome = if n > 0 {
                    // Only the winner indexes: an entry is immutable, so a
                    // losing PUT has nothing to add and a second FTS row would
                    // make the same entry match twice.
                    if let Some(index) = &index {
                        let rowid = tx.last_insert_rowid();
                        cacheindex::insert_fts(&tx, rowid, index)?;
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

    /// Prune by age and/or a total-size target (LRU eviction to reach the target).
    #[tracing::instrument(
        skip_all,
        fields(max_age_days = ?older_than_days, target_bytes = ?target_bytes)
    )]
    pub async fn cache_prune(
        &self,
        older_than_days: Option<i64>,
        target_bytes: Option<i64>,
    ) -> anyhow::Result<u64> {
        self.cache
            .write(move |conn| cache_prune(conn, older_than_days, target_bytes))
            .await
    }
}

fn cache_stats(conn: &Connection) -> anyhow::Result<CacheStatsResponse> {
    let (entries, total_bytes, oldest): (i64, i64, Option<i64>) = conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(size), 0), MIN(created_at) FROM cache_entries",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    let (hits, misses): (i64, i64) = conn.query_row(
        "SELECT hits, misses FROM cache_counters WHERE id = 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    // Its own query rather than a FILTER on the aggregate above, so the partial
    // index serves it directly — and so it costs nothing once the backfill has
    // drained, which is the steady state.
    let unindexed: i64 = conn.query_row(
        "SELECT COUNT(*) FROM cache_entries WHERE indexed_at IS NULL",
        [],
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

fn cache_prune(
    conn: &mut Connection,
    older_than_days: Option<i64>,
    target_bytes: Option<i64>,
) -> anyhow::Result<u64> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let mut pruned = 0u64;

    if let Some(days) = older_than_days {
        let cutoff = now_ms() - days * 86_400_000;
        pruned += tx.execute(
            "DELETE FROM cache_entries WHERE created_at < ?1",
            params![cutoff],
        )? as u64;
    }

    if let Some(target) = target_bytes {
        let mut total: i64 = tx.query_row(
            "SELECT COALESCE(SUM(size), 0) FROM cache_entries",
            [],
            |row| row.get(0),
        )?;
        if total > target {
            // Evict least-recently-accessed first until under target.
            let mut stmt =
                tx.prepare("SELECT key, size FROM cache_entries ORDER BY last_access_at ASC")?;
            let victims: Vec<(String, i64)> = stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<Result<_, _>>()?;
            drop(stmt);
            for (key, size) in victims {
                if total <= target {
                    break;
                }
                tx.execute("DELETE FROM cache_entries WHERE key = ?1", params![key])?;
                total -= size;
                pruned += 1;
            }
        }
    }

    tx.commit()?;
    Ok(pruned)
}
