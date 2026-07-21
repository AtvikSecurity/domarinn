//! Complexity-harness tests: drive the real runner with the scriptable
//! fake-provider binary to verify caching, retries, and concurrency behaviors
//! by counting actual provider invocations via a call-log.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use async_trait::async_trait;
use domarinn_core::cache::{
    CacheBackend, CacheEntry, CacheError, CacheKey, CacheMode, CacheStats, PurgeFilter,
};
use domarinn_core::runner::{run, RunOptions};

#[derive(Default)]
struct MemCache {
    map: Mutex<HashMap<String, CacheEntry>>,
}

#[async_trait]
impl CacheBackend for MemCache {
    async fn get(&self, key: &CacheKey) -> Result<Option<CacheEntry>, CacheError> {
        Ok(self.map.lock().unwrap().get(&key.0).cloned())
    }
    async fn put(&self, key: &CacheKey, entry: &CacheEntry) -> Result<(), CacheError> {
        self.map
            .lock()
            .unwrap()
            .entry(key.0.clone())
            .or_insert_with(|| entry.clone());
        Ok(())
    }
    async fn stats(&self) -> Result<CacheStats, CacheError> {
        Ok(CacheStats::default())
    }
    async fn purge(&self, _f: &PurgeFilter) -> Result<u64, CacheError> {
        Ok(0)
    }
}

const FAKE: &str = env!("CARGO_BIN_EXE_fake-provider");

fn count_calls(log: &Path) -> usize {
    std::fs::read_to_string(log)
        .map(|s| s.lines().count())
        .unwrap_or(0)
}

/// Build a suite using the fake-provider binary with the given env, cacheable
/// (has cache_salt) and one contains assert.
fn suite_yaml(log: &Path, mode: &str, cacheable: bool, retries: u32) -> String {
    let salt = if cacheable { "cache_salt: v1" } else { "" };
    let retry_block = if retries > 0 {
        format!("runner: {{retries: {{max: {retries}, initial_ms: 1}}}}")
    } else {
        String::new()
    };
    format!(
        r#"
version: 1
suite: harness
providers:
  - id: p
    type: exec
    command: ["{FAKE}"]
    env:
      FAKE_MODE: "{mode}"
      FAKE_CALL_LOG: "{log}"
      FAKE_OUTPUT: "hello"
    {salt}
tests:
  - id: t1
    vars: {{user_input: "hello"}}
    assert:
      - {{type: contains, value: "hello"}}
{retry_block}
"#,
        log = log.display()
    )
}

#[tokio::test]
async fn cache_hit_means_one_actual_call() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("calls.log");
    let yaml = suite_yaml(&log, "fixed", true, 0);
    let suite = domarinn_core::load_str(&yaml).unwrap();
    let cache = MemCache::default();

    run(&suite, dir.path(), &cache, None, &RunOptions::default())
        .await
        .unwrap();
    run(&suite, dir.path(), &cache, None, &RunOptions::default())
        .await
        .unwrap();

    assert_eq!(
        count_calls(&log),
        1,
        "second run should hit the cache, so the provider is invoked once"
    );
}

#[tokio::test]
async fn no_cache_calls_every_time() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("calls.log");
    let yaml = suite_yaml(&log, "fixed", true, 0);
    let suite = domarinn_core::load_str(&yaml).unwrap();
    let cache = MemCache::default();
    let opts = RunOptions {
        cache_mode: CacheMode::Disabled,
        ..Default::default()
    };
    run(&suite, dir.path(), &cache, None, &opts).await.unwrap();
    run(&suite, dir.path(), &cache, None, &opts).await.unwrap();
    assert_eq!(count_calls(&log), 2);
}

#[tokio::test]
async fn retriable_error_is_retried_then_errors() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("calls.log");
    // Always fails retriably; with max=2 retries the provider is called 3 times.
    let yaml = suite_yaml(&log, "error:retriable", false, 2);
    let suite = domarinn_core::load_str(&yaml).unwrap();
    let cache = MemCache::default();
    let result = run(&suite, dir.path(), &cache, None, &RunOptions::default())
        .await
        .unwrap();
    assert_eq!(
        result.cases[0].status,
        domarinn_core::result::CaseStatus::Error
    );
    assert_eq!(count_calls(&log), 3, "1 initial + 2 retries");
}

#[tokio::test]
async fn delayed_provider_runs_under_concurrency() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("calls.log");
    // 10 tests each delay 50ms; with concurrency 10 the whole run is fast and all
    // complete. We assert completion + call count, not timing.
    let mut yaml = format!(
        r#"
version: 1
suite: harness
providers:
  - id: p
    type: exec
    command: ["{FAKE}"]
    env: {{FAKE_MODE: "delay:50", FAKE_CALL_LOG: "{log}"}}
tests:
"#,
        log = log.display()
    );
    for i in 0..10 {
        yaml.push_str(&format!(
            "  - {{id: \"t{i}\", vars: {{user_input: \"hi\"}}, assert: [{{type: contains, value: \"hi\"}}]}}\n"
        ));
    }
    let suite = domarinn_core::load_str(&yaml).unwrap();
    let cache = MemCache::default();
    let opts = RunOptions {
        concurrency: Some(10),
        ..Default::default()
    };
    let result = run(&suite, dir.path(), &cache, None, &opts).await.unwrap();
    assert_eq!(result.cases.len(), 10);
    assert_eq!(result.summary.passed, 10);
    assert_eq!(count_calls(&log), 10);
}
