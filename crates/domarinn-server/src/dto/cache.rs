//! DTOs for `GET /cache/stats` and `POST /cache/prune`.

use serde::Serialize;
use ts_rs::TS;

/// `GET /cache/stats` response.
#[derive(Debug, Clone, Serialize, TS)]
pub struct CacheStatsResponse {
    pub entries: i64,
    pub total_bytes: i64,
    pub hits: i64,
    pub misses: i64,
    /// Entries whose body has never been examined, so their `kind`, `model`,
    /// cost and token columns are unknown and full-text search cannot reach
    /// them yet. Non-zero only while the background backfill drains a database
    /// that predates cache migration 2. The UI says "still indexing" rather
    /// than reporting those entries as having no model.
    pub unindexed: i64,
    /// RFC3339, or `None` when the cache is empty.
    pub oldest_entry_at: Option<String>,
}

/// `POST /cache/prune` response.
#[derive(Debug, Clone, Serialize, TS)]
pub struct PruneResponse {
    pub pruned: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn cache_stats_response_matches_todays_wire_shape() {
        let dto = CacheStatsResponse {
            entries: 1,
            total_bytes: 5,
            hits: 1,
            misses: 1,
            unindexed: 2,
            oldest_entry_at: Some("2026-01-01T00:00:00+00:00".to_string()),
        };
        assert_eq!(
            serde_json::to_value(&dto).unwrap(),
            json!({
                "entries": 1,
                "total_bytes": 5,
                "hits": 1,
                "misses": 1,
                "unindexed": 2,
                "oldest_entry_at": "2026-01-01T00:00:00+00:00",
            })
        );
    }

    #[test]
    fn empty_cache_has_null_oldest_entry_at() {
        let dto = CacheStatsResponse {
            entries: 0,
            total_bytes: 0,
            hits: 0,
            misses: 0,
            unindexed: 0,
            oldest_entry_at: None,
        };
        let v = serde_json::to_value(&dto).unwrap();
        assert!(v.get("oldest_entry_at").is_some());
        assert!(v["oldest_entry_at"].is_null());
    }

    #[test]
    fn prune_response_matches_todays_wire_shape() {
        let dto = PruneResponse { pruned: 3 };
        assert_eq!(serde_json::to_value(&dto).unwrap(), json!({ "pruned": 3 }));
    }
}
