//! Content-addressed cache table operations: get/has (with hit/miss accounting),
//! immutable put, stats, and age/size pruning (also used by the retention task).

use rusqlite::{params, Connection, TransactionBehavior};

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
                    tx.execute("UPDATE cache_counters SET hits = hits + 1 WHERE id = 1", [])?;
                } else {
                    tx.execute(
                        "UPDATE cache_counters SET misses = misses + 1 WHERE id = 1",
                        [],
                    )?;
                }
                tx.commit()?;
                Ok(found)
            })
            .await
    }

    pub async fn cache_put(&self, key: String, body: Vec<u8>) -> anyhow::Result<CachePutOutcome> {
        self.cache
            .write(move |conn| {
                let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
                let now = now_ms();
                let size = body.len() as i64;
                // First-write-wins: INSERT OR IGNORE, then detect whether we won.
                let n = tx.execute(
                    "INSERT OR IGNORE INTO cache_entries (key, body, size, created_at, last_access_at)
                     VALUES (?1, ?2, ?3, ?4, ?4)",
                    params![key, body, size, now],
                )?;
                tx.commit()?;
                Ok(if n > 0 {
                    CachePutOutcome::Created
                } else {
                    CachePutOutcome::Exists
                })
            })
            .await
    }

    pub async fn cache_stats(&self) -> anyhow::Result<CacheStatsResponse> {
        self.cache.read(cache_stats).await
    }

    /// Prune by age and/or a total-size target (LRU eviction to reach the target).
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
    Ok(CacheStatsResponse {
        entries,
        total_bytes,
        hits,
        misses,
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
