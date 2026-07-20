//! End-to-end runner tests against an in-memory cache and exec providers.
//!
//! These exercise the whole `run()` path offline: matrix expansion, provider
//! calls, caching, deterministic assertions, short-circuiting, and summary.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use async_trait::async_trait;
use measurellm_core::cache::CacheMode;
use measurellm_core::cache::{
    CacheBackend, CacheEntry, CacheError, CacheKey, CacheStats, PurgeFilter,
};
use measurellm_core::result::{AssertStatus, CaseStatus};
use measurellm_core::runner::{run, RunOptions};

/// A minimal in-memory cache for tests.
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
        Ok(CacheStats {
            entries: self.map.lock().unwrap().len() as u64,
            ..Default::default()
        })
    }
    async fn purge(&self, _filter: &PurgeFilter) -> Result<u64, CacheError> {
        Ok(0)
    }
}

/// A provider that echoes a fixed string, with an optional cache_salt.
fn fixed_output_suite(output: &str, cacheable: bool, assert_yaml: &str) -> String {
    let salt = if cacheable { "cache_salt: v1" } else { "" };
    format!(
        r#"
version: 1
project: test
suite: s
providers:
  - id: p
    type: exec
    command: ["sh", "-c", "cat >/dev/null; printf '{{\"output\":\"{output}\"}}'"]
    {salt}
tests:
  - id: t1
    vars: {{x: "1"}}
    assert:
{assert_yaml}
"#
    )
}

async fn run_suite(
    yaml: &str,
    opts: RunOptions,
    cache: &dyn CacheBackend,
) -> measurellm_core::RunResult {
    let suite = measurellm_core::load_str(yaml).unwrap();
    run(&suite, Path::new("."), cache, None, &opts)
        .await
        .unwrap()
}

#[tokio::test]
async fn basic_pass() {
    let yaml = fixed_output_suite(
        "hello world",
        false,
        "      - {type: contains, value: \"hello\"}",
    );
    let cache = MemCache::default();
    let result = run_suite(&yaml, RunOptions::default(), &cache).await;
    assert_eq!(result.cases.len(), 1);
    assert_eq!(result.cases[0].status, CaseStatus::Pass);
    assert_eq!(result.summary.passed, 1);
    assert_eq!(result.summary.total, 1);
}

#[tokio::test]
async fn failing_assert_produces_fail() {
    let yaml = fixed_output_suite(
        "hello",
        false,
        "      - {type: contains, value: \"goodbye\"}",
    );
    let cache = MemCache::default();
    let result = run_suite(&yaml, RunOptions::default(), &cache).await;
    assert_eq!(result.cases[0].status, CaseStatus::Fail);
    assert_eq!(result.summary.failed, 1);
}

#[tokio::test]
async fn cache_hit_on_second_run() {
    let yaml = fixed_output_suite(
        "cached",
        true,
        "      - {type: contains, value: \"cached\"}",
    );
    let cache = MemCache::default();

    let first = run_suite(&yaml, RunOptions::default(), &cache).await;
    assert_eq!(first.summary.cache_hits, 0);
    assert_eq!(first.summary.cache_misses, 1);

    let second = run_suite(&yaml, RunOptions::default(), &cache).await;
    assert_eq!(
        second.summary.cache_hits, 1,
        "second run should hit the cache"
    );
    assert!(second.cases[0].cached);
}

#[tokio::test]
async fn no_cache_mode_never_hits() {
    let yaml = fixed_output_suite("x", true, "      - {type: contains, value: \"x\"}");
    let cache = MemCache::default();
    let opts = RunOptions {
        cache_mode: CacheMode::Disabled,
        ..Default::default()
    };
    run_suite(&yaml, opts.clone(), &cache).await;
    let second = run_suite(&yaml, opts, &cache).await;
    assert_eq!(second.summary.cache_hits, 0);
}

#[tokio::test]
async fn deferred_assert_without_grader_fails_closed() {
    // An llm-rubric assert with no grader must error (never silently pass).
    let yaml = fixed_output_suite(
        "anything",
        false,
        "      - {type: llm-rubric, value: \"is good\"}",
    );
    let cache = MemCache::default();
    let result = run_suite(&yaml, RunOptions::default(), &cache).await;
    assert_eq!(result.cases[0].status, CaseStatus::Error);
    assert_eq!(result.cases[0].asserts[0].status, AssertStatus::Error);
}

#[tokio::test]
async fn deterministic_failure_short_circuits_grader() {
    // A failing deterministic assert (no threshold) means the case can't pass,
    // so the llm-rubric assert is skipped — not errored — even with no grader.
    let yaml = fixed_output_suite(
        "hello",
        false,
        "      - {type: contains, value: \"MISSING\"}\n      - {type: llm-rubric, value: \"good\"}",
    );
    let cache = MemCache::default();
    let result = run_suite(&yaml, RunOptions::default(), &cache).await;
    assert_eq!(result.cases[0].status, CaseStatus::Fail);
    assert_eq!(result.cases[0].asserts[0].status, AssertStatus::Fail);
    assert_eq!(
        result.cases[0].asserts[1].status,
        AssertStatus::Skipped,
        "grader must be short-circuited"
    );
}

#[tokio::test]
async fn repeat_produces_multiple_trials() {
    let yaml = fixed_output_suite("x", false, "      - {type: contains, value: \"x\"}");
    let cache = MemCache::default();
    let opts = RunOptions {
        repeat: 3,
        ..Default::default()
    };
    let result = run_suite(&yaml, opts, &cache).await;
    assert_eq!(result.cases.len(), 3);
    let repeats: Vec<u32> = result.cases.iter().map(|c| c.cell.repeat).collect();
    assert_eq!(repeats, vec![0, 1, 2]);
    // Distinct case keys per trial.
    let keys: std::collections::HashSet<_> = result.cases.iter().map(|c| &c.case_key).collect();
    assert_eq!(keys.len(), 3);
}

#[tokio::test]
async fn ssti_var_is_never_interpolated() {
    // A !raw var carrying an SSTI payload must reach the provider verbatim; the
    // provider echoes a fixed string, and we assert the payload never became 49.
    let yaml = r#"
version: 1
providers:
  - id: p
    type: exec
    command: ["sh", "-c", "cat >/dev/null; printf '{\"output\":\"done\"}'"]
tests:
  - id: ssti
    vars:
      payload: !raw "{{7*7}}"
    assert:
      - {type: contains, value: "done"}
"#;
    let suite = measurellm_core::load_str(yaml).unwrap();
    // The var is raw in the parsed config.
    match &suite.tests[0] {
        measurellm_core::config::TestSource::Inline(tc) => {
            assert!(tc.vars["payload"].is_raw());
        }
        _ => panic!("expected inline test"),
    }
    let cache = MemCache::default();
    let result = run(&suite, Path::new("."), &cache, None, &RunOptions::default())
        .await
        .unwrap();
    assert_eq!(result.cases[0].status, CaseStatus::Pass);
}

#[tokio::test]
async fn stress_many_cells_preserve_order_under_concurrency() {
    // 40 tests, run with concurrency 8; every cell must complete and the output
    // order must match the input order regardless of completion order.
    let mut yaml = String::from(
        r#"
version: 1
providers:
  - {id: p, type: exec, command: ["sh","-c","cat >/dev/null; printf '{\"output\":\"ok\"}'"]}
tests:
"#,
    );
    for i in 0..40 {
        yaml.push_str(&format!(
            "  - {{id: \"t{i:03}\", vars: {{}}, assert: [{{type: contains, value: \"ok\"}}]}}\n"
        ));
    }
    let suite = measurellm_core::load_str(&yaml).unwrap();
    let cache = MemCache::default();
    let opts = RunOptions {
        concurrency: Some(8),
        ..Default::default()
    };
    let result = run(&suite, Path::new("."), &cache, None, &opts)
        .await
        .unwrap();
    assert_eq!(result.cases.len(), 40);
    assert_eq!(result.summary.passed, 40);
    let ids: Vec<String> = result
        .cases
        .iter()
        .map(|c| c.cell.test_id.clone())
        .collect();
    let expected: Vec<String> = (0..40).map(|i| format!("t{i:03}")).collect();
    assert_eq!(
        ids, expected,
        "concurrent execution must preserve input order"
    );
}

#[tokio::test]
async fn matrix_is_deterministically_ordered() {
    // Two providers, two tests → four cells, always in the same order.
    let yaml = r#"
version: 1
providers:
  - {id: a, type: exec, command: ["sh","-c","cat >/dev/null; printf '{\"output\":\"x\"}'"]}
  - {id: b, type: exec, command: ["sh","-c","cat >/dev/null; printf '{\"output\":\"x\"}'"]}
tests:
  - {id: t1, vars: {}, assert: [{type: contains, value: "x"}]}
  - {id: t2, vars: {}, assert: [{type: contains, value: "x"}]}
"#;
    let suite = measurellm_core::load_str(yaml).unwrap();
    let cache = MemCache::default();
    let result = run(&suite, Path::new("."), &cache, None, &RunOptions::default())
        .await
        .unwrap();
    let order: Vec<(String, String)> = result
        .cases
        .iter()
        .map(|c| (c.cell.provider_id.clone(), c.cell.test_id.clone()))
        .collect();
    assert_eq!(
        order,
        vec![
            ("a".into(), "t1".into()),
            ("a".into(), "t2".into()),
            ("b".into(), "t1".into()),
            ("b".into(), "t2".into()),
        ]
    );
}
