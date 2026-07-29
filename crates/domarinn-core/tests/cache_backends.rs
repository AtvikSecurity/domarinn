//! Every cache backend, exercised through a real `run()`.
//!
//! The backends have unit tests of their own, but those poke `get`/`put`
//! directly. Nothing checked that a *run* behaves identically whichever store is
//! behind it — which is the only thing a user experiences, and where the
//! interesting failures live: a remote that populates the local tier, a remote
//! that is down, a bucket prefix that should isolate two teams, concurrent cases
//! racing one key.
//!
//! `domarinn-core` cannot depend on `domarinn-cache` (that would be a cycle), so
//! the layered/S3/HTTP shapes are reconstructed here from the same primitives
//! the real backends use. What is under test is the *runner's* contract with any
//! backend, so a faithful stand-in is the right level: the concrete stores are
//! covered where they live, in `crates/domarinn-cache`.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use domarinn_core::cache::{
    CacheBackend, CacheEntry, CacheError, CacheKey, CacheMode, CacheStats, PurgeFilter,
};
use domarinn_core::result::CaseStatus;
use domarinn_core::runner::{run, RunOptions};
use domarinn_core::RunResult;

// ── Stand-ins for the real stores ────────────────────────────────────────────

/// First-write-wins in-memory store, counting the traffic it sees.
///
/// The counters are what make "the local tier absorbed this" assertable; a plain
/// map can only show the end state, not who was asked.
#[derive(Default)]
struct MemCache {
    map: Mutex<HashMap<String, CacheEntry>>,
    gets: AtomicUsize,
    puts: AtomicUsize,
}

impl MemCache {
    fn entries(&self) -> usize {
        self.map.lock().unwrap().len()
    }
    fn gets(&self) -> usize {
        self.gets.load(Ordering::SeqCst)
    }
    fn seed(&self, key: &CacheKey, entry: &CacheEntry) {
        self.map
            .lock()
            .unwrap()
            .insert(key.0.clone(), entry.clone());
    }
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
            entries: self.entries() as u64,
            ..Default::default()
        })
    }
    async fn purge(&self, _filter: &PurgeFilter) -> Result<u64, CacheError> {
        Ok(0)
    }
}

/// A read-through pair, mirroring `domarinn_cache::LayeredCache`: local first,
/// remote on a miss, populate local from a remote hit, best-effort remote write.
struct Layered {
    local: Arc<MemCache>,
    remote: Arc<dyn CacheBackend>,
}

#[async_trait]
impl CacheBackend for Layered {
    async fn get(&self, key: &CacheKey) -> Result<Option<CacheEntry>, CacheError> {
        if let Some(entry) = self.local.get(key).await? {
            return Ok(Some(entry));
        }
        match self.remote.get(key).await {
            Ok(Some(entry)) => {
                let _ = self.local.put(key, &entry).await;
                Ok(Some(entry))
            }
            Ok(None) => Ok(None),
            // A remote outage degrades to a miss rather than failing the run.
            Err(_) => Ok(None),
        }
    }
    async fn put(&self, key: &CacheKey, entry: &CacheEntry) -> Result<(), CacheError> {
        self.local.put(key, entry).await?;
        let _ = self.remote.put(key, entry).await;
        Ok(())
    }
    async fn stats(&self) -> Result<CacheStats, CacheError> {
        self.local.stats().await
    }
    async fn purge(&self, filter: &PurgeFilter) -> Result<u64, CacheError> {
        self.local.purge(filter).await
    }
}

/// A remote that is down. Every operation errors.
struct DeadRemote;

#[async_trait]
impl CacheBackend for DeadRemote {
    async fn get(&self, _key: &CacheKey) -> Result<Option<CacheEntry>, CacheError> {
        Err(CacheError(anyhow::anyhow!("connection refused")))
    }
    async fn put(&self, _key: &CacheKey, _entry: &CacheEntry) -> Result<(), CacheError> {
        Err(CacheError(anyhow::anyhow!("connection refused")))
    }
    async fn stats(&self) -> Result<CacheStats, CacheError> {
        Err(CacheError(anyhow::anyhow!("connection refused")))
    }
    async fn purge(&self, _filter: &PurgeFilter) -> Result<u64, CacheError> {
        Ok(0)
    }
}

/// A prefixed view over one shared store, standing in for two teams pointed at
/// one bucket with different `cache.s3.prefix` values.
struct Prefixed {
    inner: Arc<MemCache>,
    prefix: &'static str,
}

impl Prefixed {
    fn scoped(&self, key: &CacheKey) -> CacheKey {
        CacheKey(format!("{}/{}", self.prefix, key.0))
    }
}

#[async_trait]
impl CacheBackend for Prefixed {
    async fn get(&self, key: &CacheKey) -> Result<Option<CacheEntry>, CacheError> {
        self.inner.get(&self.scoped(key)).await
    }
    async fn put(&self, key: &CacheKey, entry: &CacheEntry) -> Result<(), CacheError> {
        self.inner.put(&self.scoped(key), entry).await
    }
    async fn stats(&self) -> Result<CacheStats, CacheError> {
        self.inner.stats().await
    }
    async fn purge(&self, filter: &PurgeFilter) -> Result<u64, CacheError> {
        self.inner.purge(filter).await
    }
}

// ── Suites ───────────────────────────────────────────────────────────────────

fn suite(cases: usize) -> String {
    let mut tests = String::new();
    for i in 0..cases {
        tests.push_str(&format!("  - id: case-{i}\n    vars: {{x: \"{i}\"}}\n"));
    }
    format!(
        r#"
version: 1
project: test
suite: backends
providers:
  - id: p
    type: exec
    command: ["sh", "-c", "cat >/dev/null; printf '{{\"output\":\"ok\"}}'"]
    cache_salt: "v1"
tests:
{tests}"#
    )
}

async fn run_with(yaml: &str, cache: &dyn CacheBackend, opts: RunOptions) -> RunResult {
    let suite = domarinn_core::load_str(yaml).unwrap();
    run(&suite, Path::new("."), cache, None, &opts)
        .await
        .unwrap()
}

async fn run_default(yaml: &str, cache: &dyn CacheBackend) -> RunResult {
    run_with(yaml, cache, RunOptions::default()).await
}

// ── Every backend behaves the same from the outside ──────────────────────────

/// Builds one of the backend shapes under test. A fresh store per shape, so
/// the four cannot warm each other.
type BuildBackend = Box<dyn Fn() -> Arc<dyn CacheBackend>>;

/// Cold pays, warm is free, and a third run stays free — for each shape.
///
/// Parameterised rather than four near-identical tests because the property
/// *is* "identical whichever store is behind it", and writing it once is the
/// only way that claim can be checked rather than asserted.
#[tokio::test]
async fn cold_then_warm_behaves_identically_on_every_backend() {
    let shapes: Vec<(&str, BuildBackend)> = vec![
        ("local only", Box::new(|| Arc::new(MemCache::default()))),
        (
            "layered over a shared remote",
            Box::new(|| {
                Arc::new(Layered {
                    local: Arc::new(MemCache::default()),
                    remote: Arc::new(MemCache::default()),
                })
            }),
        ),
        (
            "layered over a prefixed bucket",
            Box::new(|| {
                Arc::new(Layered {
                    local: Arc::new(MemCache::default()),
                    remote: Arc::new(Prefixed {
                        inner: Arc::new(MemCache::default()),
                        prefix: "team-a",
                    }),
                })
            }),
        ),
        (
            "layered over a dead remote",
            Box::new(|| {
                Arc::new(Layered {
                    local: Arc::new(MemCache::default()),
                    remote: Arc::new(DeadRemote),
                })
            }),
        ),
    ];

    for (name, build) in shapes {
        let cache = build();
        let yaml = suite(3);

        let cold = run_default(&yaml, cache.as_ref()).await;
        assert_eq!(cold.summary.cache_misses, 3, "{name}: a cold run pays");
        assert_eq!(cold.summary.cache_hits, 0, "{name}");

        let warm = run_default(&yaml, cache.as_ref()).await;
        assert_eq!(warm.summary.cache_hits, 3, "{name}: a warm run is free");

        let again = run_default(&yaml, cache.as_ref()).await;
        assert_eq!(again.summary.cache_hits, 3, "{name}: and stays free");
        assert!(
            again.cases.iter().all(|c| c.status != CaseStatus::Error),
            "{name}: no case may error"
        );
    }
}

#[tokio::test]
async fn a_remote_hit_populates_the_local_tier() {
    // The point of a shared cache: the first teammate pays, everyone else gets a
    // hit — and pays the network only once, because the answer lands locally.
    let seeder = Arc::new(MemCache::default());
    let yaml = suite(2);

    // Teammate one, whose writes reach the shared remote.
    let first = Layered {
        local: Arc::new(MemCache::default()),
        remote: seeder.clone(),
    };
    run_default(&yaml, &first).await;
    assert_eq!(seeder.entries(), 2, "the shared remote holds both answers");

    // Teammate two: cold local, warm remote.
    let local = Arc::new(MemCache::default());
    let second = Layered {
        local: local.clone(),
        remote: seeder.clone(),
    };
    let warm = run_default(&yaml, &second).await;
    assert_eq!(warm.summary.cache_hits, 2, "the remote answers");
    assert_eq!(local.entries(), 2, "and the local tier is populated");

    let remote_gets_before = seeder.gets();
    let again = run_default(&yaml, &second).await;
    assert_eq!(again.summary.cache_hits, 2);
    assert_eq!(
        seeder.gets(),
        remote_gets_before,
        "a locally-warm rerun must not touch the network at all"
    );
}

#[tokio::test]
async fn a_dead_remote_degrades_to_local_rather_than_failing_the_run() {
    // An outage in a shared cache must cost speed, never correctness. Before
    // anything else this has to *not error*: a run that fails because a cache
    // was unreachable turns an optimisation into a dependency.
    let local = Arc::new(MemCache::default());
    let cache = Layered {
        local: local.clone(),
        remote: Arc::new(DeadRemote),
    };
    let yaml = suite(2);

    let cold = run_default(&yaml, &cache).await;
    assert!(cold.cases.iter().all(|c| c.status != CaseStatus::Error));
    assert_eq!(
        local.entries(),
        2,
        "the local tier still records the answers"
    );

    let warm = run_default(&yaml, &cache).await;
    assert_eq!(warm.summary.cache_hits, 2, "and still serves them");
}

#[tokio::test]
async fn two_prefixes_over_one_bucket_do_not_share() {
    // `cache.s3.prefix` is how two teams share a bucket without sharing a cache.
    // If it leaked, one team's answers would silently stand in for another's.
    let bucket = Arc::new(MemCache::default());
    let yaml = suite(2);

    let team_a = Prefixed {
        inner: bucket.clone(),
        prefix: "team-a",
    };
    let team_b = Prefixed {
        inner: bucket.clone(),
        prefix: "team-b",
    };

    run_default(&yaml, &team_a).await;
    let b_cold = run_default(&yaml, &team_b).await;
    assert_eq!(
        b_cold.summary.cache_hits, 0,
        "a different prefix is a different cache"
    );
    assert_eq!(bucket.entries(), 4, "both prefixes coexist in one bucket");

    let a_warm = run_default(&yaml, &team_a).await;
    assert_eq!(
        a_warm.summary.cache_hits, 2,
        "and neither disturbs the other"
    );
}

// ── Concurrency, modes, and forward compatibility ────────────────────────────

#[tokio::test]
async fn a_real_disk_store_survives_concurrent_cases_racing_one_key() {
    // Two cases with identical vars share a key by design, and `--repeat` and
    // high concurrency put several of them in flight at once. The disk backend
    // writes to a unique temp file and renames, so the race is safe — but the
    // property worth pinning is the observable one: exactly one entry, readable,
    // no torn files.
    let dir = tempfile::tempdir().unwrap();
    let cache = DiskLike::new(dir.path());
    // Every case has the same var, so all of them collide on one key.
    let yaml = r#"
version: 1
project: test
suite: backends
runner: {concurrency: 16}
providers:
  - id: p
    type: exec
    command: ["sh", "-c", "cat >/dev/null; printf '{\"output\":\"ok\"}'"]
    cache_salt: "v1"
tests:
  - id: a
    vars: {x: "same"}
  - id: b
    vars: {x: "same"}
  - id: c
    vars: {x: "same"}
  - id: d
    vars: {x: "same"}
"#;

    let result = run_with(
        yaml,
        &cache,
        RunOptions {
            repeat: 4,
            ..Default::default()
        },
    )
    .await;
    assert!(result.cases.iter().all(|c| c.status != CaseStatus::Error));
    // One entry per repeat index; the four colliding cases share each one.
    assert_eq!(
        cache.entries(),
        4,
        "identical vars share a key, so only the repeat index separates them"
    );
}

/// A file-backed store with the same atomic-rename discipline as
/// `domarinn_cache::LocalDiskCache`, so the concurrency test above exercises
/// real filesystem behaviour rather than a `Mutex`.
struct DiskLike {
    root: std::path::PathBuf,
}

impl DiskLike {
    fn new(root: &Path) -> Self {
        DiskLike {
            root: root.to_path_buf(),
        }
    }
    fn path_for(&self, key: &CacheKey) -> std::path::PathBuf {
        let hex = key.0.strip_prefix("sha256:").unwrap_or(&key.0);
        self.root.join(format!("{hex}.json"))
    }
    fn entries(&self) -> usize {
        std::fs::read_dir(&self.root)
            .map(|rd| {
                rd.flatten()
                    .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
                    .count()
            })
            .unwrap_or(0)
    }
}

#[async_trait]
impl CacheBackend for DiskLike {
    async fn get(&self, key: &CacheKey) -> Result<Option<CacheEntry>, CacheError> {
        match std::fs::read(self.path_for(key)) {
            Ok(bytes) => {
                Ok(Some(serde_json::from_slice(&bytes).map_err(|e| {
                    CacheError(anyhow::anyhow!("corrupt entry: {e}"))
                })?))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(CacheError(anyhow::anyhow!("{e}"))),
        }
    }
    async fn put(&self, key: &CacheKey, entry: &CacheEntry) -> Result<(), CacheError> {
        let path = self.path_for(key);
        if path.exists() {
            return Ok(());
        }
        std::fs::create_dir_all(&self.root).ok();
        let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
        let bytes = serde_json::to_vec(entry).map_err(|e| CacheError(anyhow::anyhow!("{e}")))?;
        std::fs::write(&tmp, &bytes).map_err(|e| CacheError(anyhow::anyhow!("{e}")))?;
        // Rename over an existing file is fine: both writers wrote a valid
        // answer to the same question.
        std::fs::rename(&tmp, &path).map_err(|e| CacheError(anyhow::anyhow!("{e}")))?;
        Ok(())
    }
    async fn stats(&self) -> Result<CacheStats, CacheError> {
        Ok(CacheStats {
            entries: self.entries() as u64,
            ..Default::default()
        })
    }
    async fn purge(&self, _filter: &PurgeFilter) -> Result<u64, CacheError> {
        Ok(0)
    }
}

#[tokio::test]
async fn cache_only_replays_from_a_shared_remote() {
    // Fully offline CI: no credentials, no live calls, everything replayed from
    // whatever a previous job pushed to the shared store. A miss is an
    // infrastructure error rather than a quiet network call.
    let shared = Arc::new(MemCache::default());
    let yaml = suite(2);

    let seeding = Layered {
        local: Arc::new(MemCache::default()),
        remote: shared.clone(),
    };
    run_default(&yaml, &seeding).await;

    // A fresh job: empty local tier, warm remote, strict mode.
    let fresh = Layered {
        local: Arc::new(MemCache::default()),
        remote: shared.clone(),
    };
    let strict = run_with(
        &yaml,
        &fresh,
        RunOptions {
            cache_mode: CacheMode::ReadOnlyStrict,
            ..Default::default()
        },
    )
    .await;
    assert_eq!(strict.summary.cache_hits, 2);
    assert!(strict.cases.iter().all(|c| c.status != CaseStatus::Error));
}

#[tokio::test]
async fn cache_only_errors_when_the_shared_remote_is_cold() {
    let cache = Layered {
        local: Arc::new(MemCache::default()),
        remote: Arc::new(MemCache::default()),
    };
    let strict = run_with(
        &suite(1),
        &cache,
        RunOptions {
            cache_mode: CacheMode::ReadOnlyStrict,
            ..Default::default()
        },
    )
    .await;
    assert_eq!(strict.cases[0].status, CaseStatus::Error);
    assert!(strict.cases[0]
        .error
        .as_deref()
        .unwrap_or_default()
        .contains("cache-only"));
}

#[tokio::test]
async fn an_entry_from_a_newer_domarinn_replays_through_any_backend() {
    // A shared bucket is read by whatever version each teammate and CI job
    // happens to be running. An entry written by a newer binary carries fields
    // this one has never heard of, and skipping them must be a no-op rather than
    // a hard error that fails the run — or one upgrade anywhere poisons the
    // cache for everyone behind it.
    let from_the_future = serde_json::json!({
        "created_at": "2026-01-01T00:00:00Z",
        "provider_fingerprint": {"type": "exec"},
        "output": "ok",
        "domarinn_version": "99.0.0",
        "program_digest": "blake3:deadbeef",
        "a_field_invented_after_this_binary_shipped": {"nested": true},
    });
    let entry: CacheEntry = serde_json::from_value(from_the_future).unwrap();

    // Seed it under the key this run will actually ask for, discovered by
    // running once against a throwaway store.
    let yaml = suite(1);
    let scratch = MemCache::default();
    run_default(&yaml, &scratch).await;
    let key = {
        let map = scratch.map.lock().unwrap();
        CacheKey(map.keys().next().unwrap().clone())
    };

    let store = Arc::new(MemCache::default());
    store.seed(&key, &entry);
    let layered = Layered {
        local: Arc::new(MemCache::default()),
        remote: store.clone(),
    };

    let result = run_default(&yaml, &layered).await;
    assert_eq!(result.summary.cache_hits, 1);
    assert_eq!(result.cases[0].output.as_ref().unwrap().as_text(), "ok");
    assert_eq!(result.cases[0].status, CaseStatus::Pass);
}

#[tokio::test]
async fn no_cache_mode_neither_reads_nor_writes_any_backend() {
    let local = Arc::new(MemCache::default());
    let remote = Arc::new(MemCache::default());
    let cache = Layered {
        local: local.clone(),
        remote: remote.clone(),
    };
    let opts = RunOptions {
        cache_mode: CacheMode::Disabled,
        ..Default::default()
    };

    run_with(&suite(2), &cache, opts.clone()).await;
    let second = run_with(&suite(2), &cache, opts).await;

    assert_eq!(second.summary.cache_hits, 0);
    assert_eq!(local.entries(), 0);
    assert_eq!(remote.entries(), 0);
    assert_eq!(local.gets(), 0, "--no-cache must not even look");
}
