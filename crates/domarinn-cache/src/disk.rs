//! Content-addressed local disk cache: one file per entry, atomic writes.
//!
//! Layout: `<root>/<first2hex>/<hex>.json`. Each entry is written to a unique
//! temp file and atomically renamed, so concurrent runs (and `s3 sync`) can
//! never observe a torn file — there is no single shared JSON blob to clobber.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use domarinn_core::cache::{
    CacheBackend, CacheEntry, CacheEnumerate, CacheError, CacheKey, CacheStats, Enumerated,
    EnumeratedEntry, PurgeFilter,
};

/// A cache rooted at a directory on the local filesystem.
pub struct LocalDiskCache {
    root: PathBuf,
}

impl LocalDiskCache {
    /// Create a cache rooted at `root` (created lazily on first write).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        LocalDiskCache { root: root.into() }
    }

    /// The default project-local cache directory, `.domarinn/cache`.
    pub fn default_project() -> Self {
        LocalDiskCache::new(PathBuf::from(".domarinn").join("cache"))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn path_for(&self, key: &CacheKey) -> PathBuf {
        // key is "sha256:<hex>"; use the hex for the on-disk layout.
        let hex = key.0.strip_prefix("sha256:").unwrap_or(&key.0);
        let shard = &hex[..hex.len().min(2)];
        self.root.join(shard).join(format!("{hex}.json"))
    }
}

#[async_trait]
impl CacheEnumerate for LocalDiskCache {
    /// Walk the shard tree, stat every entry file, and return the newest
    /// `limit` of them.
    ///
    /// Two phases, and the order matters. Everything is stat'ed first and the
    /// result is sorted by mtime before truncating, so cutting the list drops
    /// the *oldest* entries. Truncating during the walk instead would drop
    /// whichever entries `read_dir` happened to name last, which is arbitrary
    /// and would make two calls disagree about what exists.
    ///
    /// A missing root is an empty cache, not an error: the directory is created
    /// lazily on first write, so a suite that has never run has nothing here.
    async fn enumerate(&self, limit: usize) -> Result<Enumerated, CacheError> {
        let mut found: Vec<EnumeratedEntry> = Vec::new();
        let mut scanned = 0usize;

        let mut shards = match tokio::fs::read_dir(&self.root).await {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Enumerated::default()),
            Err(e) => return Err(CacheError(anyhow::anyhow!("reading {:?}: {e}", self.root))),
        };
        while let Some(shard) = shards
            .next_entry()
            .await
            .map_err(|e| CacheError(anyhow::anyhow!("iterating shards: {e}")))?
        {
            if !shard.file_type().await.map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let mut files = tokio::fs::read_dir(shard.path())
                .await
                .map_err(|e| CacheError(anyhow::anyhow!("reading shard: {e}")))?;
            while let Some(file) = files
                .next_entry()
                .await
                .map_err(|e| CacheError(anyhow::anyhow!("iterating shard: {e}")))?
            {
                let name = file.file_name();
                let name = name.to_string_lossy();
                // `.json` only, which also skips the `.tmp-*` files a concurrent
                // writer leaves in flight — a run writing this cache while it is
                // being browsed is the normal case, not a race to guard against.
                let Some(hex) = name.strip_suffix(".json") else {
                    continue;
                };
                scanned += 1;
                let key = CacheKey(format!("sha256:{hex}"));
                if !CacheKey::is_valid(&key.0) {
                    continue;
                }
                let Ok(meta) = file.metadata().await else {
                    continue;
                };
                found.push(EnumeratedEntry {
                    key,
                    size: meta.len(),
                    modified: meta
                        .modified()
                        .ok()
                        .map(chrono::DateTime::<chrono::Utc>::from),
                });
            }
        }

        found.sort_by(|a, b| {
            b.modified
                .cmp(&a.modified)
                .then_with(|| a.key.0.cmp(&b.key.0))
        });
        let truncated = found.len() > limit;
        found.truncate(limit);
        Ok(Enumerated {
            entries: found,
            truncated,
            scanned,
        })
    }
}

#[async_trait]
impl CacheBackend for LocalDiskCache {
    async fn get(&self, key: &CacheKey) -> Result<Option<CacheEntry>, CacheError> {
        let path = self.path_for(key);
        match tokio::fs::read(&path).await {
            Ok(bytes) => {
                let entry = serde_json::from_slice(&bytes).map_err(|e| {
                    CacheError(anyhow::anyhow!("corrupt cache entry {path:?}: {e}"))
                })?;
                Ok(Some(entry))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(CacheError(anyhow::anyhow!("reading {path:?}: {e}"))),
        }
    }

    async fn put(&self, key: &CacheKey, entry: &CacheEntry) -> Result<(), CacheError> {
        let path = self.path_for(key);
        // Immutable: first write wins.
        if tokio::fs::try_exists(&path).await.unwrap_or(false) {
            return Ok(());
        }
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| CacheError(anyhow::anyhow!("creating {parent:?}: {e}")))?;
        }
        let bytes = serde_json::to_vec(entry)
            .map_err(|e| CacheError(anyhow::anyhow!("serializing entry: {e}")))?;

        // Unique temp name so concurrent writers never collide.
        let tmp = path.with_extension(format!("tmp-{}", unique_suffix()));
        tokio::fs::write(&tmp, &bytes)
            .await
            .map_err(|e| CacheError(anyhow::anyhow!("writing {tmp:?}: {e}")))?;
        match tokio::fs::rename(&tmp, &path).await {
            Ok(()) => Ok(()),
            Err(e) => {
                // Another writer may have won the race; that is fine (immutable).
                let _ = tokio::fs::remove_file(&tmp).await;
                if tokio::fs::try_exists(&path).await.unwrap_or(false) {
                    Ok(())
                } else {
                    Err(CacheError(anyhow::anyhow!("renaming into {path:?}: {e}")))
                }
            }
        }
    }

    async fn stats(&self) -> Result<CacheStats, CacheError> {
        let mut stats = CacheStats::default();
        let mut shards = match tokio::fs::read_dir(&self.root).await {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(stats),
            Err(e) => return Err(CacheError(anyhow::anyhow!("reading {:?}: {e}", self.root))),
        };
        while let Some(shard) = shards
            .next_entry()
            .await
            .map_err(|e| CacheError(anyhow::anyhow!("iterating shards: {e}")))?
        {
            if !shard.file_type().await.map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let mut files = tokio::fs::read_dir(shard.path())
                .await
                .map_err(|e| CacheError(anyhow::anyhow!("reading shard: {e}")))?;
            while let Some(file) = files
                .next_entry()
                .await
                .map_err(|e| CacheError(anyhow::anyhow!("iterating shard: {e}")))?
            {
                let name = file.file_name();
                if !name.to_string_lossy().ends_with(".json") {
                    continue;
                }
                if let Ok(meta) = file.metadata().await {
                    stats.entries += 1;
                    stats.total_bytes += meta.len();
                }
            }
        }
        Ok(stats)
    }

    async fn purge(&self, filter: &PurgeFilter) -> Result<u64, CacheError> {
        let cutoff = filter.older_than.map(|d| chrono::Utc::now() - d);
        let mut removed = 0u64;
        let mut shards = match tokio::fs::read_dir(&self.root).await {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(e) => return Err(CacheError(anyhow::anyhow!("reading {:?}: {e}", self.root))),
        };
        while let Some(shard) = shards
            .next_entry()
            .await
            .map_err(|e| CacheError(anyhow::anyhow!("iterating shards: {e}")))?
        {
            if !shard.file_type().await.map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let mut files = tokio::fs::read_dir(shard.path())
                .await
                .map_err(|e| CacheError(anyhow::anyhow!("reading shard: {e}")))?;
            while let Some(file) = files
                .next_entry()
                .await
                .map_err(|e| CacheError(anyhow::anyhow!("iterating shard: {e}")))?
            {
                let path = file.path();
                if path.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                let should_remove = match cutoff {
                    None => true,
                    Some(cutoff) => file
                        .metadata()
                        .await
                        .ok()
                        .and_then(|m| m.modified().ok())
                        .map(|mtime| chrono::DateTime::<chrono::Utc>::from(mtime) < cutoff)
                        .unwrap_or(false),
                };
                if should_remove && tokio::fs::remove_file(&path).await.is_ok() {
                    removed += 1;
                }
            }
        }
        Ok(removed)
    }
}

/// A process-and-call-unique suffix for temp files. Uses the address of a stack
/// local plus a monotonic counter — no dependency on wall-clock or rng (both of
/// which complicate determinism/testing elsewhere).
fn unique_suffix() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    format!("{pid}-{n}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use domarinn_core::cache::CacheKey;
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
            domarinn_version: "test".into(),
        }
    }

    #[tokio::test]
    async fn put_then_get_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let cache = LocalDiskCache::new(dir.path());
        let key = CacheKey::compute(&json!({"a": 1}));
        assert!(cache.get(&key).await.unwrap().is_none());
        cache.put(&key, &sample_entry()).await.unwrap();
        let got = cache.get(&key).await.unwrap().unwrap();
        assert_eq!(got.output, Output::Text("hi".into()));
    }

    #[tokio::test]
    async fn put_is_immutable_first_write_wins() {
        let dir = tempfile::tempdir().unwrap();
        let cache = LocalDiskCache::new(dir.path());
        let key = CacheKey::compute(&json!({"a": 1}));
        cache.put(&key, &sample_entry()).await.unwrap();
        let mut second = sample_entry();
        second.output = Output::Text("changed".into());
        cache.put(&key, &second).await.unwrap();
        let got = cache.get(&key).await.unwrap().unwrap();
        assert_eq!(
            got.output,
            Output::Text("hi".into()),
            "first write should win"
        );
    }

    #[tokio::test]
    async fn stats_counts_entries() {
        let dir = tempfile::tempdir().unwrap();
        let cache = LocalDiskCache::new(dir.path());
        for i in 0..3 {
            let key = CacheKey::compute(&json!({ "a": i }));
            cache.put(&key, &sample_entry()).await.unwrap();
        }
        let stats = cache.stats().await.unwrap();
        assert_eq!(stats.entries, 3);
        assert!(stats.total_bytes > 0);
    }

    /// Truncation must drop the oldest, not an arbitrary slice. `read_dir`
    /// order is arbitrary, so a walk that stopped early would return whichever
    /// entries the filesystem named first and two calls could disagree about
    /// what the cache contains.
    #[tokio::test]
    async fn enumerating_past_the_limit_drops_the_oldest_entries() {
        let dir = tempfile::tempdir().unwrap();
        let cache = LocalDiskCache::new(dir.path());

        // Written oldest-first, with distinct mtimes.
        let mut keys = Vec::new();
        for i in 0..5 {
            let key = CacheKey::compute(&json!({ "i": i }));
            cache.put(&key, &sample_entry()).await.unwrap();
            keys.push(key);
            tokio::time::sleep(std::time::Duration::from_millis(12)).await;
        }

        let all = cache.enumerate(100).await.unwrap();
        assert_eq!(all.entries.len(), 5);
        assert!(!all.truncated);
        assert_eq!(all.entries[0].key, keys[4], "newest must lead");

        let page = cache.enumerate(2).await.unwrap();
        assert!(page.truncated);
        assert_eq!(
            page.entries.iter().map(|e| &e.key).collect::<Vec<_>>(),
            vec![&keys[4], &keys[3]],
            "the two newest, not two arbitrary entries"
        );
    }

    /// A cache directory that has never been written to is an empty cache. The
    /// root is created lazily on first write, so this is the ordinary state of
    /// a suite that has not run yet — not a misconfiguration.
    #[tokio::test]
    async fn enumerating_a_root_that_does_not_exist_is_empty_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let cache = LocalDiskCache::new(dir.path().join("never-written"));
        let found = cache.enumerate(10).await.unwrap();
        assert!(found.entries.is_empty());
        assert!(!found.truncated);
    }

    /// A run writing this cache while it is being browsed is the normal case.
    /// In-flight temp files must not surface as entries.
    #[tokio::test]
    async fn a_writers_temp_file_is_not_an_entry() {
        let dir = tempfile::tempdir().unwrap();
        let cache = LocalDiskCache::new(dir.path());
        let key = CacheKey::compute(&json!({ "a": 1 }));
        cache.put(&key, &sample_entry()).await.unwrap();

        let shard = dir
            .path()
            .join(&key.0.strip_prefix("sha256:").unwrap()[..2]);
        tokio::fs::write(shard.join(".tmp-123-4"), b"half written")
            .await
            .unwrap();

        let found = cache.enumerate(10).await.unwrap();
        assert_eq!(found.entries.len(), 1);
        assert_eq!(found.entries[0].key, key);
    }

    #[tokio::test]
    async fn concurrent_writers_do_not_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        let cache = std::sync::Arc::new(LocalDiskCache::new(dir.path()));
        let key = CacheKey::compute(&json!({"race": true}));
        let mut handles = Vec::new();
        for _ in 0..16 {
            let cache = cache.clone();
            let key = key.clone();
            handles.push(tokio::spawn(async move {
                cache.put(&key, &sample_entry()).await.unwrap();
            }));
        }
        for h in handles {
            h.await.unwrap();
        }
        // Exactly one readable entry, no torn files.
        assert!(cache.get(&key).await.unwrap().is_some());
        assert_eq!(cache.stats().await.unwrap().entries, 1);
    }
}
