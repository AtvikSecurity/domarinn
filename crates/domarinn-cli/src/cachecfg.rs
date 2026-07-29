//! Building the cache backend for a run from the suite `cache:` config.
//!
//! `disk` uses the local content-addressed cache. `layered` wraps it in a
//! read-through [`LayeredCache`] over a shared remote — S3 when `cache.s3` is
//! set, else the HTTP results server. `http` and `s3` are deprecated aliases
//! that name one of those tiers outright. A missing server URL or credentials
//! degrades to local disk with a warning, never a hard failure.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use domarinn_cache::{
    LayeredCache, LocalDiskCache, ReadOnlyCache, RemoteHttpCache, S3Cache, S3Config,
};
use domarinn_core::cache::CacheBackend;
use domarinn_core::config::{CacheBackendKind, Suite};

/// Environment fallback for `--cache-dir`.
pub const CACHE_DIR_ENV: &str = "DOMARINN_CACHE_DIR";

/// The cache directory a run should use, and any older location worth reading.
pub struct LocalRoot {
    /// Where entries are read from and written to.
    pub root: PathBuf,
    /// A directory a previous version would have used, if it exists and differs.
    /// Read-only: entries found there are copied forward, never written back.
    pub legacy: Option<PathBuf>,
}

/// Resolve where the local cache lives.
///
/// Precedence is `--cache-dir`, then `DOMARINN_CACHE_DIR`, then `.domarinn/cache`
/// beside the suite.
///
/// Beside the *suite*, not beside the process. Every other path in a run —
/// `file://`, `$digest:`, an `exec` child's cwd — resolves against the suite
/// directory, and the cache was the one exception: running `domarinn run
/// evals/s.yaml` from a repo root and `domarinn run s.yaml` from `evals/` used
/// two different caches for identical work, so whichever you did second paid in
/// full. Anchoring it to the suite makes the cache a property of what is being
/// evaluated rather than of where you were standing.
///
/// That relocation would strand every entry written under the old rule, so a
/// cwd-relative `.domarinn/cache` that still exists is offered as a read-only
/// legacy tier — see [`build_cache`].
pub fn local_root(flag: Option<&Path>, base_dir: &Path) -> LocalRoot {
    let explicit = flag.map(PathBuf::from).or_else(|| {
        std::env::var_os(CACHE_DIR_ENV)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    });
    if let Some(root) = explicit {
        // An explicit directory is an explicit answer; reading some other
        // directory as well would be second-guessing it.
        return LocalRoot { root, legacy: None };
    }

    let root = base_dir.join(".domarinn").join("cache");
    let cwd_relative = PathBuf::from(".domarinn").join("cache");
    // Compared canonically: running from the suite directory makes the two the
    // same place by different names, and layering a directory over itself would
    // double every lookup for nothing.
    let same = match (root.canonicalize(), cwd_relative.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    };
    let legacy = (!same && cwd_relative.is_dir()).then_some(cwd_relative);
    LocalRoot { root, legacy }
}

/// Build the cache backend for a run.
pub fn build_cache(
    suite: &Suite,
    server_url: Option<&str>,
    local_root: &LocalRoot,
) -> Arc<dyn CacheBackend> {
    let mut local: Arc<dyn CacheBackend> = Arc::new(LocalDiskCache::new(&local_root.root));
    if let Some(legacy) = &local_root.legacy {
        tracing::debug!(
            legacy = %legacy.display(),
            root = %local_root.root.display(),
            "reading the previous cwd-relative cache directory as a read-only tier"
        );
        // Read-through, exactly as a shared remote is: a hit down there is
        // copied into the new root, so the old directory is drained rather than
        // depended on, and nothing is ever written back into it.
        local = Arc::new(LayeredCache::new(
            local,
            Arc::new(ReadOnlyCache::new(Arc::new(LocalDiskCache::new(legacy)))),
        ));
    }

    let backend = suite
        .cache
        .as_ref()
        .map(|c| c.backend.clone())
        .unwrap_or_default();
    warn_if_deprecated(&backend);

    match remote_tier(
        &backend,
        suite.cache.as_ref().and_then(|c| c.s3.as_ref()).is_some(),
    ) {
        RemoteTier::None => local,
        RemoteTier::Http => layer_remote(local, server_url),
        RemoteTier::S3 => layer_s3(local, suite),
    }
}

/// The shared tier a `backend:` selection resolves to.
///
/// Named separately because `http` and `s3` are deprecated *aliases* of
/// `layered`: what an alias promises is that the spelling does not decide
/// behavior, this mapping does — and a mapping is something a test can hold to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RemoteTier {
    /// Local disk only.
    None,
    /// The HTTP results server.
    Http,
    /// An S3-compatible object store.
    S3,
}

#[allow(deprecated)]
fn remote_tier(backend: &CacheBackendKind, has_s3_cfg: bool) -> RemoteTier {
    match backend {
        CacheBackendKind::Disk => RemoteTier::None,
        CacheBackendKind::Http => RemoteTier::Http,
        CacheBackendKind::S3 => RemoteTier::S3,
        // The one selection that looks at the suite: a `cache.s3` block is the
        // user saying which remote they meant.
        CacheBackendKind::Layered => {
            if has_s3_cfg {
                RemoteTier::S3
            } else {
                RemoteTier::Http
            }
        }
    }
}

#[allow(deprecated)]
fn warn_if_deprecated(backend: &CacheBackendKind) {
    match backend {
        CacheBackendKind::Http => tracing::warn!(
            "`backend: http` is a deprecated alias for `layered`; it behaves identically and will be removed"
        ),
        CacheBackendKind::S3 => tracing::warn!(
            "`backend: s3` is a deprecated alias for `layered`; it behaves identically and will be removed"
        ),
        CacheBackendKind::Disk | CacheBackendKind::Layered => {}
    }
}

fn layer_remote(local: Arc<dyn CacheBackend>, server_url: Option<&str>) -> Arc<dyn CacheBackend> {
    let url = server_url
        .map(String::from)
        .or_else(|| std::env::var("DOMARINN_SERVER_URL").ok())
        .filter(|s| !s.is_empty());
    let token = std::env::var("DOMARINN_TOKEN")
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
                "cache backend needs a server URL (--server-url / DOMARINN_SERVER_URL); using local disk"
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The headline: where you stand does not decide which cache you get.
    #[test]
    fn the_default_root_is_beside_the_suite() {
        let resolved = local_root(None, Path::new("/repo/evals"));
        assert_eq!(resolved.root, PathBuf::from("/repo/evals/.domarinn/cache"));
    }

    #[test]
    fn an_explicit_flag_wins_and_reads_nothing_else() {
        let resolved = local_root(Some(Path::new("/ci/restored")), Path::new("/repo/evals"));
        assert_eq!(resolved.root, PathBuf::from("/ci/restored"));
        assert!(
            resolved.legacy.is_none(),
            "an explicit directory must not be silently layered over another"
        );
    }

    /// A suite directory with no stray `.domarinn/cache` under the process cwd
    /// has nothing to migrate, so no legacy tier is attached.
    #[test]
    fn no_legacy_tier_when_there_is_nothing_there() {
        let dir = tempfile::tempdir().unwrap();
        let resolved = local_root(None, dir.path());
        assert_eq!(resolved.root, dir.path().join(".domarinn").join("cache"));
        assert!(resolved.legacy.is_none());
    }

    /// What "deprecated alias" has to mean: the same tier, for both the suite
    /// that names a remote and the suite that has none to name.
    #[test]
    #[allow(deprecated)]
    fn the_deprecated_aliases_choose_the_same_tier_layered_would() {
        for has_s3_cfg in [false, true] {
            assert_eq!(
                remote_tier(&CacheBackendKind::Http, has_s3_cfg),
                RemoteTier::Http,
                "`http` names the HTTP tier outright, `cache.s3` or not"
            );
            assert_eq!(
                remote_tier(&CacheBackendKind::S3, has_s3_cfg),
                RemoteTier::S3,
                "`s3` names the S3 tier outright — a missing `cache.s3` block \
                 degrades to disk with an s3-specific warning rather than \
                 falling back to HTTP"
            );
        }
        // ...and `layered` is exactly those two, chosen by the config.
        assert_eq!(
            remote_tier(&CacheBackendKind::Layered, false),
            remote_tier(&CacheBackendKind::Http, false)
        );
        assert_eq!(
            remote_tier(&CacheBackendKind::Layered, true),
            remote_tier(&CacheBackendKind::S3, true)
        );
    }

    /// The default is local-only: a suite with no `cache:` block never reaches
    /// for a remote, so it cannot be degraded by one.
    #[test]
    fn disk_is_local_only() {
        assert_eq!(
            remote_tier(&CacheBackendKind::Disk, true),
            RemoteTier::None,
            "`disk` ignores a `cache.s3` block rather than being upgraded by it"
        );
    }
}
