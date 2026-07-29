//! A backend that serves reads and quietly discards writes.
//!
//! Composed under [`crate::LayeredCache`] to drain a store rather than depend on
//! it: reads fall through and populate the tier in front, writes stop here. The
//! motivating case is a cache directory a previous version of domarinn wrote to.
//! Its entries are worth reading — they cost real money — but writing new ones
//! there would keep the old location alive forever instead of letting it empty
//! out and be deleted.

use async_trait::async_trait;
use domarinn_core::cache::{
    CacheBackend, CacheEntry, CacheError, CacheKey, CacheStats, PurgeFilter,
};
use std::sync::Arc;

/// Wraps a backend so only its reads are reachable.
pub struct ReadOnlyCache {
    inner: Arc<dyn CacheBackend>,
}

impl ReadOnlyCache {
    pub fn new(inner: Arc<dyn CacheBackend>) -> Self {
        ReadOnlyCache { inner }
    }
}

#[async_trait]
impl CacheBackend for ReadOnlyCache {
    async fn get(&self, key: &CacheKey) -> Result<Option<CacheEntry>, CacheError> {
        self.inner.get(key).await
    }

    /// Silently accepted, never performed.
    ///
    /// `Ok(())` rather than an error because the caller is
    /// [`crate::LayeredCache`], whose remote writes are best-effort: an error
    /// would be logged as a failure on every single write, which is noise about
    /// a thing that is working exactly as intended.
    async fn put(&self, _key: &CacheKey, _entry: &CacheEntry) -> Result<(), CacheError> {
        Ok(())
    }

    async fn stats(&self) -> Result<CacheStats, CacheError> {
        self.inner.stats().await
    }

    /// Refused, and reported as zero purged. Pruning a store this wrapper exists
    /// to protect from writes would be the most surprising possible reading of
    /// "read only".
    async fn purge(&self, _filter: &PurgeFilter) -> Result<u64, CacheError> {
        Ok(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use domarinn_core::types::Output;
    use serde_json::json;

    fn entry(text: &str) -> CacheEntry {
        CacheEntry {
            tool_calls: Vec::new(),
            created_at: chrono::Utc::now(),
            provider_fingerprint: Some(json!({"type": "exec"})),
            request: None,
            output: Output::Text(text.into()),
            usage: None,
            cost_usd: None,
            stop_reason: None,
            model: None,
            verdict: None,
            raw: None,
            reasoning: None,
            empty_reason: None,
            attempts: None,
            provider_latency_ms: None,
            program_digest: None,
            domarinn_version: "test".into(),
        }
    }

    #[tokio::test]
    async fn reads_pass_through_but_writes_do_not() {
        let dir = tempfile::tempdir().unwrap();
        let inner = Arc::new(crate::LocalDiskCache::new(dir.path()));
        let key = CacheKey::compute(&json!({"a": 1}));
        inner.put(&key, &entry("seeded")).await.unwrap();

        let ro = ReadOnlyCache::new(inner.clone());
        assert_eq!(
            ro.get(&key).await.unwrap().unwrap().output,
            Output::Text("seeded".into())
        );

        let other = CacheKey::compute(&json!({"b": 2}));
        ro.put(&other, &entry("written")).await.unwrap();
        assert!(
            inner.get(&other).await.unwrap().is_none(),
            "a write must not reach the wrapped store"
        );
    }

    #[tokio::test]
    async fn purge_is_refused_rather_than_forwarded() {
        let dir = tempfile::tempdir().unwrap();
        let inner = Arc::new(crate::LocalDiskCache::new(dir.path()));
        let key = CacheKey::compute(&json!({"a": 1}));
        inner.put(&key, &entry("keep me")).await.unwrap();

        let ro = ReadOnlyCache::new(inner.clone());
        assert_eq!(ro.purge(&PurgeFilter::default()).await.unwrap(), 0);
        assert!(inner.get(&key).await.unwrap().is_some());
    }
}
