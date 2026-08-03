//! A two-tier read-through cache: a fast local backend in front of a shared
//! remote one.
//!
//! - `get`: try local; on a miss, try remote and — if that hits — populate local
//!   before returning, so the next read is served locally.
//! - `put`: write local synchronously (authoritative); write remote best-effort
//!   (errors are logged, never propagated).
//! - `stats` / `purge`: operate on the local tier only.
//!
//! The remote tier is treated as *best-effort* on both reads and writes: a
//! remote outage degrades the layered cache to "local only" rather than failing
//! the caller.

use std::sync::Arc;

use async_trait::async_trait;
use domarinn_core::cache::{
    CacheBackend, CacheEntry, CacheError, CacheKey, CacheStats, PurgeFilter,
};

/// Wraps a `(local, remote)` pair of backends as a single read-through cache.
pub struct LayeredCache {
    local: Arc<dyn CacheBackend>,
    remote: Arc<dyn CacheBackend>,
}

impl LayeredCache {
    /// Create a layered cache from a local (fast, authoritative) and a remote
    /// (shared, best-effort) backend.
    pub fn new(local: Arc<dyn CacheBackend>, remote: Arc<dyn CacheBackend>) -> Self {
        LayeredCache { local, remote }
    }
}

#[async_trait]
impl CacheBackend for LayeredCache {
    async fn get(&self, key: &CacheKey) -> Result<Option<CacheEntry>, CacheError> {
        // Fast path: a local hit never touches the remote tier.
        if let Some(entry) = self.local.get(key).await? {
            return Ok(Some(entry));
        }
        // Miss: consult remote. A remote error is non-fatal — degrade to a miss.
        match self.remote.get(key).await {
            Ok(Some(entry)) => {
                // Read-through: populate local so subsequent reads are fast.
                // A populate failure must not fail the read.
                if let Err(e) = self.local.put(key, &entry).await {
                    tracing::warn!(error = %e, "layered cache: failed to populate local from remote");
                }
                Ok(Some(entry))
            }
            Ok(None) => Ok(None),
            Err(e) => {
                tracing::warn!(error = %e, "layered cache: remote get failed, treating as miss");
                Ok(None)
            }
        }
    }

    async fn put(&self, key: &CacheKey, entry: &CacheEntry) -> Result<(), CacheError> {
        // Local write is authoritative and synchronous.
        self.local.put(key, entry).await?;
        // Remote write is best-effort; swallow errors with a warning.
        if let Err(e) = self.remote.put(key, entry).await {
            tracing::warn!(error = %e, "layered cache: remote put failed (ignored)");
        }
        Ok(())
    }

    async fn stats(&self) -> Result<CacheStats, CacheError> {
        self.local.stats().await
    }

    async fn purge(&self, filter: &PurgeFilter) -> Result<u64, CacheError> {
        self.local.purge(filter).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    use domarinn_core::types::Output;
    use serde_json::json;

    fn sample_entry() -> CacheEntry {
        CacheEntry {
            kind: None,
            tool_calls: Vec::new(),
            created_at: chrono::Utc::now(),
            provider_fingerprint: Some(json!({"type": "exec"})),
            request: None,
            output: Output::Text("hi".into()),
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
            address: None,
            domarinn_version: "test".into(),
        }
    }

    /// An in-memory backend that counts get/put calls, for asserting which tier
    /// a layered operation touches. Writes are first-write-wins (immutable),
    /// mirroring the real backends.
    #[derive(Default)]
    struct MemCache {
        map: Mutex<HashMap<String, CacheEntry>>,
        gets: AtomicUsize,
        puts: AtomicUsize,
    }

    #[async_trait]
    impl CacheBackend for MemCache {
        async fn get(&self, key: &CacheKey) -> Result<Option<CacheEntry>, CacheError> {
            self.gets.fetch_add(1, Ordering::SeqCst);
            Ok(self.map.lock().unwrap().get(&key.0).cloned())
        }
        async fn put(&self, key: &CacheKey, entry: &CacheEntry) -> Result<(), CacheError> {
            self.puts.fetch_add(1, Ordering::SeqCst);
            self.map
                .lock()
                .unwrap()
                .entry(key.0.clone())
                .or_insert_with(|| entry.clone());
            Ok(())
        }
        async fn stats(&self) -> Result<CacheStats, CacheError> {
            Ok(CacheStats {
                entries: self.map.lock().unwrap().len() as u64,
                ..Default::default()
            })
        }
        async fn purge(&self, _filter: &PurgeFilter) -> Result<u64, CacheError> {
            let mut m = self.map.lock().unwrap();
            let n = m.len() as u64;
            m.clear();
            Ok(n)
        }
    }

    /// A backend whose `get` always errors — used to prove remote failures are
    /// non-fatal on read.
    struct FailingGet;

    #[async_trait]
    impl CacheBackend for FailingGet {
        async fn get(&self, _key: &CacheKey) -> Result<Option<CacheEntry>, CacheError> {
            Err(CacheError(anyhow::anyhow!("remote is down")))
        }
        async fn put(&self, _key: &CacheKey, _entry: &CacheEntry) -> Result<(), CacheError> {
            Err(CacheError(anyhow::anyhow!("remote is down")))
        }
        async fn stats(&self) -> Result<CacheStats, CacheError> {
            Ok(CacheStats::default())
        }
        async fn purge(&self, _filter: &PurgeFilter) -> Result<u64, CacheError> {
            Ok(0)
        }
    }

    #[tokio::test]
    async fn read_through_populates_local_from_remote() {
        let local = Arc::new(MemCache::default());
        let remote = Arc::new(MemCache::default());
        let key = CacheKey::compute(&json!({"a": 1}));
        // Seed only the remote tier.
        remote.put(&key, &sample_entry()).await.unwrap();

        let layered = LayeredCache::new(local.clone(), remote.clone());
        let got = layered.get(&key).await.unwrap().unwrap();
        assert_eq!(got.output, Output::Text("hi".into()));

        // Remote was consulted exactly once...
        assert_eq!(remote.gets.load(Ordering::SeqCst), 1);
        // ...and local is now populated for the next read.
        assert!(local.map.lock().unwrap().contains_key(&key.0));
    }

    /// Promotion must not rewrite a kind this binary has never heard of.
    ///
    /// This is the concrete hazard [`domarinn_core::cache::EntryKind`] is an
    /// open newtype to avoid. Promotion deserializes a remote entry and `put`s
    /// the *deserialized value*, so with a closed enum plus `#[serde(other)]` an
    /// older binary would collapse a newer kind to its catch-all and then write
    /// that back — corrupting the local tier of every machine that read through
    /// it. The local tier here is a real `LocalDiskCache` rather than the
    /// in-memory double, because `MemCache` clones entries and would never
    /// exercise the serialization step where the loss would happen.
    #[tokio::test]
    async fn promoting_a_remote_hit_preserves_an_unknown_kind() {
        let dir = tempfile::tempdir().unwrap();
        let local = Arc::new(crate::LocalDiskCache::new(dir.path()));
        let remote = Arc::new(MemCache::default());
        let key = CacheKey::compute(&json!({"a": 1}));

        let mut entry = sample_entry();
        entry.kind = Some(domarinn_core::cache::EntryKind::new("distillation"));
        remote.put(&key, &entry).await.unwrap();

        let layered = LayeredCache::new(local.clone(), remote.clone());
        layered.get(&key).await.unwrap().unwrap();

        // Read back through the disk tier alone: this is the copy a later run
        // finds, and the only one that went through serialization.
        let promoted = local.get(&key).await.unwrap().unwrap();
        assert_eq!(
            promoted.kind.as_ref().map(|k| k.as_str()),
            Some("distillation"),
            "promotion rewrote a kind it did not recognize"
        );
    }

    #[tokio::test]
    async fn local_hit_does_not_touch_remote() {
        let local = Arc::new(MemCache::default());
        let remote = Arc::new(MemCache::default());
        let key = CacheKey::compute(&json!({"a": 1}));
        // Seed only the local tier.
        local.put(&key, &sample_entry()).await.unwrap();

        let layered = LayeredCache::new(local.clone(), remote.clone());
        let got = layered.get(&key).await.unwrap().unwrap();
        assert_eq!(got.output, Output::Text("hi".into()));

        // The remote tier must be untouched on a local hit.
        assert_eq!(remote.gets.load(Ordering::SeqCst), 0);
        assert_eq!(remote.puts.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn put_writes_both_tiers() {
        let local = Arc::new(MemCache::default());
        let remote = Arc::new(MemCache::default());
        let key = CacheKey::compute(&json!({"a": 1}));

        let layered = LayeredCache::new(local.clone(), remote.clone());
        layered.put(&key, &sample_entry()).await.unwrap();

        assert!(local.map.lock().unwrap().contains_key(&key.0));
        assert!(remote.map.lock().unwrap().contains_key(&key.0));
    }

    #[tokio::test]
    async fn remote_get_failure_degrades_to_miss() {
        let local = Arc::new(MemCache::default());
        let remote = Arc::new(FailingGet);
        let key = CacheKey::compute(&json!({"a": 1}));

        let layered = LayeredCache::new(local.clone(), remote);
        // Local miss + remote error => treated as a clean miss, not an error.
        assert!(layered.get(&key).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn remote_put_failure_is_swallowed() {
        let local = Arc::new(MemCache::default());
        let remote = Arc::new(FailingGet);
        let key = CacheKey::compute(&json!({"a": 1}));

        let layered = LayeredCache::new(local.clone(), remote);
        // Remote put errors, but the local write succeeds => Ok overall.
        layered.put(&key, &sample_entry()).await.unwrap();
        assert!(local.map.lock().unwrap().contains_key(&key.0));
    }

    #[tokio::test]
    async fn stats_and_purge_target_local() {
        let local = Arc::new(MemCache::default());
        let remote = Arc::new(MemCache::default());
        let key = CacheKey::compute(&json!({"a": 1}));
        local.put(&key, &sample_entry()).await.unwrap();
        remote.put(&key, &sample_entry()).await.unwrap();

        let layered = LayeredCache::new(local.clone(), remote.clone());
        assert_eq!(layered.stats().await.unwrap().entries, 1);
        assert_eq!(layered.purge(&PurgeFilter::default()).await.unwrap(), 1);
        // Purge hit local only; remote is untouched.
        assert!(local.map.lock().unwrap().is_empty());
        assert!(remote.map.lock().unwrap().contains_key(&key.0));
    }
}
