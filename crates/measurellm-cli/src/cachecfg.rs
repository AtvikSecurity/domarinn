//! Building the cache backend for a run from the suite `cache:` config.
//!
//! `disk` uses the local content-addressed cache. `http`/`s3`/`layered` wrap the
//! local cache in a read-through [`LayeredCache`] over a shared remote. A missing
//! server URL or credentials degrades to local disk with a warning, never a hard
//! failure.

use std::sync::Arc;

use measurellm_cache::{LayeredCache, LocalDiskCache, RemoteHttpCache, S3Cache, S3Config};
use measurellm_core::cache::CacheBackend;
use measurellm_core::config::{CacheBackendKind, Suite};

/// Build the cache backend for a run.
pub fn build_cache(suite: &Suite, server_url: Option<&str>) -> Arc<dyn CacheBackend> {
    let local: Arc<dyn CacheBackend> = Arc::new(LocalDiskCache::default_project());
    let backend = suite
        .cache
        .as_ref()
        .map(|c| c.backend.clone())
        .unwrap_or_default();

    match backend {
        CacheBackendKind::Disk => local,
        CacheBackendKind::Http => layer_remote(local, server_url),
        CacheBackendKind::S3 => layer_s3(local, suite),
        CacheBackendKind::Layered => {
            if suite.cache.as_ref().and_then(|c| c.s3.as_ref()).is_some() {
                layer_s3(local, suite)
            } else {
                layer_remote(local, server_url)
            }
        }
    }
}

fn layer_remote(local: Arc<dyn CacheBackend>, server_url: Option<&str>) -> Arc<dyn CacheBackend> {
    let url = server_url
        .map(String::from)
        .or_else(|| std::env::var("MEASURELLM_SERVER_URL").ok())
        .filter(|s| !s.is_empty());
    let token = std::env::var("MEASURELLM_TOKEN")
        .ok()
        .filter(|s| !s.is_empty());
    match url {
        Some(u) => match RemoteHttpCache::new(&u, token) {
            Ok(remote) => Arc::new(LayeredCache::new(local, Arc::new(remote))),
            Err(e) => {
                tracing::warn!(error = %e, backend = "http", "remote cache unavailable; using local disk");
                local
            }
        },
        None => {
            tracing::warn!(
                backend = "http",
                "cache backend needs a server URL (--server-url / MEASURELLM_SERVER_URL); using local disk"
            );
            local
        }
    }
}

fn layer_s3(local: Arc<dyn CacheBackend>, suite: &Suite) -> Arc<dyn CacheBackend> {
    let s3 = match suite.cache.as_ref().and_then(|c| c.s3.as_ref()) {
        Some(s) => s,
        None => {
            tracing::warn!(
                backend = "s3",
                "cache backend needs a cache.s3 config; using local disk"
            );
            return local;
        }
    };
    let config = S3Config {
        bucket: s3.bucket.clone(),
        endpoint: s3.endpoint.clone(),
        region: s3.region.clone(),
        prefix: s3.prefix.clone().unwrap_or_default(),
        force_path_style: s3.force_path_style,
    };
    match S3Cache::new(config) {
        Ok(remote) => Arc::new(LayeredCache::new(local, Arc::new(remote))),
        Err(e) => {
            tracing::warn!(error = %e, backend = "s3", "S3 cache unavailable; using local disk");
            local
        }
    }
}
