//! Cache backend backed by any S3-compatible object store.
//!
//! Uses the `object_store` crate so the same code targets AWS S3, MinIO,
//! Garage, SeaweedFS, and friends. Object layout:
//!
//! ```text
//! {prefix}/v1/{first2hex}/{key}.json
//! ```
//!
//! where `{key}` is the full content-addressed key (`sha256:<hex>`) and
//! `{first2hex}` shards the keyspace by the first two hex digits.
//!
//! ## Additive-only, first write wins
//!
//! Two writers who agree on a key agree on the *answer*, because the key is a
//! hash of the question. They do not agree on the bytes: a [`CacheEntry`]
//! carries `created_at`, `attempts`, `provider_latency_ms` and
//! `domarinn_version`, all of which describe the call rather than the response.
//!
//! This module used to claim otherwise, and used the claim to justify an
//! unconditional `PUT`. The claim was false and so was the conclusion: a plain
//! `PUT` made a shared bucket last-write-wins while [`crate::LocalDiskCache`]
//! and the results server are both first-write-wins, so the `attempts` and
//! latency a run replayed could change under it depending on who wrote last.
//!
//! So `put` checks for the object first and skips when it is already there.
//! That restores first-write-wins across every backend and removes the write
//! amplification of re-uploading entries a shared bucket already holds. It is
//! deliberately a `HEAD` rather than a conditional put (`PutMode::Create` /
//! `If-None-Match: *`), which MinIO does not support and generic S3 endpoints do
//! not universally enable.
//!
//! The check is advisory, not a lock: two writers racing a cold key can both
//! see "absent" and both upload. That is the pre-existing behaviour and it is
//! harmless — they are writing interchangeable answers to the same question.
//!
//! Nothing is ever deleted; retention is left to bucket lifecycle policies.

use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use domarinn_core::cache::{
    CacheBackend, CacheEntry, CacheError, CacheKey, CacheStats, PurgeFilter,
};
use object_store::{aws::AmazonS3Builder, path::Path as ObjectPath, ObjectStore, ObjectStoreExt};

/// Non-secret configuration for an S3-compatible bucket.
///
/// Credentials are intentionally *not* here: they are read from the standard
/// AWS environment/instance chain (env vars, profile, IMDS, ...) by
/// `object_store`.
#[derive(Debug, Clone, Default)]
pub struct S3Config {
    /// Target bucket name.
    pub bucket: String,
    /// Optional custom endpoint, e.g. `http://minio:9000` for non-AWS stores.
    pub endpoint: Option<String>,
    /// Optional region (some S3-compatible stores accept any value).
    pub region: Option<String>,
    /// Key prefix within the bucket (may be empty). Leading/trailing slashes are
    /// ignored.
    pub prefix: String,
    /// When true, use path-style addressing (`endpoint/bucket/key`) rather than
    /// virtual-hosted-style. MinIO/Garage typically need this.
    pub force_path_style: bool,
}

/// A cache stored in an S3-compatible bucket.
pub struct S3Cache {
    store: Arc<dyn ObjectStore>,
    prefix: String,
}

impl S3Cache {
    /// Build a cache targeting a real S3-compatible endpoint from [`S3Config`].
    ///
    /// Credentials come from the standard AWS chain via `object_store`.
    pub fn new(config: S3Config) -> Result<Self, CacheError> {
        let mut builder = AmazonS3Builder::from_env().with_bucket_name(&config.bucket);
        if let Some(endpoint) = &config.endpoint {
            builder = builder.with_endpoint(endpoint);
            // Local/self-hosted endpoints are commonly plain HTTP.
            if endpoint.starts_with("http://") {
                builder = builder.with_allow_http(true);
            }
        }
        if let Some(region) = &config.region {
            builder = builder.with_region(region);
        }
        // force_path_style == true  => path-style addressing
        //                            => virtual-hosted-style request must be OFF.
        builder = builder.with_virtual_hosted_style_request(!config.force_path_style);

        let store = builder
            .build()
            .map_err(|e| CacheError(anyhow::anyhow!("building S3 store: {e}")))?;
        Ok(S3Cache::with_store(Arc::new(store), config.prefix))
    }

    /// Construct from an already-built object store.
    ///
    /// This is the injection seam used by tests, which point at
    /// `object_store::local::LocalFileSystem` to exercise real get/put behavior
    /// without any network or S3.
    pub fn with_store(store: Arc<dyn ObjectStore>, prefix: impl Into<String>) -> Self {
        S3Cache {
            store,
            prefix: normalize_prefix(prefix.into()),
        }
    }

    fn location(&self, key: &CacheKey) -> ObjectPath {
        // key is "sha256:<hex>"; shard on the first two hex digits.
        let hex = key.0.strip_prefix("sha256:").unwrap_or(&key.0);
        let shard = &hex[..hex.len().min(2)];
        let raw = if self.prefix.is_empty() {
            format!("v1/{shard}/{}.json", key.0)
        } else {
            format!("{}/v1/{shard}/{}.json", self.prefix, key.0)
        };
        ObjectPath::from(raw)
    }
}

fn normalize_prefix(prefix: String) -> String {
    prefix.trim_matches('/').to_string()
}

#[async_trait]
impl CacheBackend for S3Cache {
    #[tracing::instrument(level = "debug", skip(self), fields(key = %key))]
    async fn get(&self, key: &CacheKey) -> Result<Option<CacheEntry>, CacheError> {
        let loc = self.location(key);
        match self.store.get(&loc).await {
            Ok(result) => {
                let body: Bytes = result
                    .bytes()
                    .await
                    .map_err(|e| CacheError(anyhow::anyhow!("reading {loc}: {e}")))?;
                let entry = serde_json::from_slice(&body)
                    .map_err(|e| CacheError(anyhow::anyhow!("corrupt cache entry {loc}: {e}")))?;
                Ok(Some(entry))
            }
            Err(object_store::Error::NotFound { .. }) => Ok(None),
            Err(e) => Err(CacheError(anyhow::anyhow!("reading {loc}: {e}"))),
        }
    }

    #[tracing::instrument(level = "debug", skip(self, entry), fields(key = %key))]
    async fn put(&self, key: &CacheKey, entry: &CacheEntry) -> Result<(), CacheError> {
        let loc = self.location(key);
        // First write wins — see the module docs. A `HEAD` costs one round trip
        // against a bucket that already has the entry, which is cheaper than the
        // upload it replaces.
        match self.store.head(&loc).await {
            Ok(_) => return Ok(()),
            Err(object_store::Error::NotFound { .. }) => {}
            // Any other failure is inconclusive about whether the object is
            // there. Fall through and write: a redundant upload is a far better
            // outcome than dropping a response somebody paid for.
            Err(e) => {
                tracing::debug!(error = %e, %loc, "existence check failed; writing anyway");
            }
        }
        let bytes = serde_json::to_vec(entry)
            .map_err(|e| CacheError(anyhow::anyhow!("serializing entry: {e}")))?;
        self.store
            .put(&loc, bytes.into())
            .await
            .map_err(|e| CacheError(anyhow::anyhow!("writing {loc}: {e}")))?;
        Ok(())
    }

    async fn stats(&self) -> Result<CacheStats, CacheError> {
        // Computing real stats would require a full (paginated) LIST of the
        // bucket, which is expensive and rate-limited on large stores. Callers
        // that need stats use the local tier (see `LayeredCache::stats`), so we
        // return an empty summary here rather than incurring that cost.
        Ok(CacheStats::default())
    }

    async fn purge(&self, _filter: &PurgeFilter) -> Result<u64, CacheError> {
        // No-op by design: S3 retention/eviction is managed by bucket lifecycle
        // policies, not by this client. Entries are immutable and additive.
        Ok(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use domarinn_core::types::Output;
    use object_store::local::LocalFileSystem;
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

    /// A local-filesystem-backed object store standing in for S3 in tests.
    fn local_store() -> (tempfile::TempDir, Arc<dyn ObjectStore>) {
        let dir = tempfile::tempdir().unwrap();
        let fs = LocalFileSystem::new_with_prefix(dir.path()).unwrap();
        (dir, Arc::new(fs))
    }

    #[tokio::test]
    async fn put_then_get_round_trips() {
        let (_dir, store) = local_store();
        let cache = S3Cache::with_store(store, "cache");
        let key = CacheKey::compute(&json!({"a": 1}));

        assert!(cache.get(&key).await.unwrap().is_none());
        cache.put(&key, &sample_entry()).await.unwrap();
        let got = cache.get(&key).await.unwrap().unwrap();
        assert_eq!(got.output, Output::Text("hi".into()));
    }

    /// First write wins, matching the local disk backend and the results server.
    ///
    /// The second entry is deliberately *different* — which is the whole point.
    /// Two writers of one key agree on the answer but not on the bytes, because
    /// an entry records `attempts`, `created_at` and the writer's version. When
    /// this was an unconditional `PUT` the last writer's metadata replaced the
    /// first's, so a shared bucket quietly disagreed with every other backend.
    #[tokio::test]
    async fn a_second_write_does_not_replace_the_first() {
        let (_dir, store) = local_store();
        let cache = S3Cache::with_store(store, "cache");
        let key = CacheKey::compute(&json!({"a": 1}));

        cache.put(&key, &sample_entry()).await.unwrap();
        let mut second = sample_entry();
        second.output = Output::Text("from another writer".into());
        second.attempts = Some(7);
        cache.put(&key, &second).await.unwrap();

        let got = cache.get(&key).await.unwrap().unwrap();
        assert_eq!(got.output, Output::Text("hi".into()), "first write wins");
        assert_eq!(got.attempts, None);
    }

    #[tokio::test]
    async fn empty_prefix_still_round_trips() {
        let (_dir, store) = local_store();
        let cache = S3Cache::with_store(store, "");
        let key = CacheKey::compute(&json!({"b": 2}));

        cache.put(&key, &sample_entry()).await.unwrap();
        assert!(cache.get(&key).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn missing_key_is_none() {
        let (_dir, store) = local_store();
        let cache = S3Cache::with_store(store, "cache");
        let key = CacheKey::compute(&json!({"never": "written"}));
        assert!(cache.get(&key).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn purge_is_noop_and_stats_are_empty() {
        let (_dir, store) = local_store();
        let cache = S3Cache::with_store(store, "cache");
        cache
            .put(&CacheKey::compute(&json!({"a": 1})), &sample_entry())
            .await
            .unwrap();
        assert_eq!(cache.purge(&PurgeFilter::default()).await.unwrap(), 0);
        assert_eq!(cache.stats().await.unwrap().entries, 0);
    }
}
