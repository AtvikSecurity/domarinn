//! Unit tests for the `runner:` knobs that shape provider calls —
//! `rate_limit`, `timeout_ms` and `short_circuit`. A sibling of
//! `runner_tests.rs` rather than more of it, to keep both under the per-file
//! line ratchet; still a private child of the runner (`use super::*`).

use super::runner_cache::{call_with_cache, CacheCall, CacheRunState};
use super::*;
use crate::cache::{CacheEntry, CacheError, CacheKey, CacheStats, PurgeFilter};
use crate::provider::{ProviderError, ProviderResponse, TestMeta};
use crate::rate_limit::RateLimiter;
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::Duration;

/// An in-memory, first-write-wins store — enough for a miss to become a hit.
#[derive(Default)]
struct MemCache(Mutex<HashMap<String, CacheEntry>>);

#[async_trait]
impl CacheBackend for MemCache {
    async fn get(&self, key: &CacheKey) -> Result<Option<CacheEntry>, CacheError> {
        Ok(self.0.lock().unwrap().get(&key.0).cloned())
    }
    async fn put(&self, key: &CacheKey, entry: &CacheEntry) -> Result<(), CacheError> {
        self.0
            .lock()
            .unwrap()
            .entry(key.0.clone())
            .or_insert_with(|| entry.clone());
        Ok(())
    }
    async fn stats(&self) -> Result<CacheStats, CacheError> {
        Ok(CacheStats::default())
    }
    async fn purge(&self, _filter: &PurgeFilter) -> Result<u64, CacheError> {
        Ok(0)
    }
}

/// A cacheable in-process provider that counts its live calls. In-process on
/// purpose: paused time auto-advances whenever the runtime is idle, which a
/// real child process or socket would turn into a spurious timeout.
#[derive(Default)]
struct CountingProvider {
    calls: AtomicU32,
}

#[async_trait]
impl Provider for CountingProvider {
    fn id(&self) -> &str {
        "counting"
    }
    fn fingerprint(&self) -> Json {
        serde_json::json!({ "type": "counting" })
    }
    fn canonical_request(&self, req: &ProviderRequest) -> Option<Json> {
        Some(serde_json::json!({ "vars": req.vars }))
    }
    async fn call(
        &self,
        _req: &ProviderRequest,
        _ctx: &CallCtx,
    ) -> Result<ProviderResponse, ProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(ProviderResponse::text("ok"))
    }
}

fn request(input: &str) -> ProviderRequest {
    ProviderRequest {
        tools: Vec::new(),
        prompt: None,
        vars: BTreeMap::from([("input".to_string(), Json::String(input.into()))]),
        params: serde_json::Map::new(),
        test: TestMeta::default(),
        case_salt: None,
    }
}

/// The headline property of where the limiter sits: a replayed answer is not a
/// request, so it must not spend a slot. If the hit below consumed one, the
/// next miss would wait two intervals instead of one.
#[tokio::test(start_paused = true)]
async fn a_cache_hit_spends_no_rate_limit_slot() {
    let provider = CountingProvider::default();
    let cache = MemCache::default();
    let state = CacheRunState::default();
    let policy = crate::empty_policy::EmptyPolicy::default();
    let limiter = RateLimiter::per_second(1.0).unwrap();
    let retry = RetryPolicy::default();
    let call = CacheCall {
        backend: &cache,
        mode: CacheMode::ReadWrite,
        repeat: 0,
        state: &state,
        policy: &policy,
        probe_legacy: false,
    };
    let ctx = CallCtx::default();
    let start = tokio::time::Instant::now();

    let first = call_with_cache(&provider, &request("a"), &ctx, call, &retry, Some(&limiter))
        .await
        .unwrap();
    assert!(!first.cached);
    let replay = call_with_cache(&provider, &request("a"), &ctx, call, &retry, Some(&limiter))
        .await
        .unwrap();
    assert!(replay.cached, "the second identical call is a hit");
    assert_eq!(start.elapsed(), Duration::ZERO, "a hit never waits");

    call_with_cache(&provider, &request("b"), &ctx, call, &retry, Some(&limiter))
        .await
        .unwrap();
    assert_eq!(
        start.elapsed(),
        Duration::from_secs(1),
        "the next miss waits one interval after the first — the hit took no slot"
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
}

/// A suite whose `runner:` block is `runner_yaml`, with one exec provider.
fn suite_with_runner(runner_yaml: &str) -> crate::config::Suite {
    crate::load_str(&format!(
        "version: 1\nproviders: [{{id: p, type: exec, command: [\"true\"]}}]\n\
         tests: [{{vars: {{x: 1}}, assert: [{{type: contains, value: x}}]}}]\n\
         runner: {runner_yaml}\n"
    ))
    .unwrap()
}

/// `validate` refuses these on the CLI path; a library caller of `run` gets
/// the same refusal as a config error instead of a run that cannot progress.
#[tokio::test]
async fn run_refuses_a_rate_limit_or_timeout_that_cannot_work() {
    for (runner, path) in [
        ("{rate_limit: {rps: 0}}", "runner.rate_limit.rps"),
        ("{rate_limit: {rps: -2}}", "runner.rate_limit.rps"),
        ("{timeout_ms: 0}", "runner.timeout_ms"),
    ] {
        let suite = suite_with_runner(runner);
        let err = run(
            &suite,
            Path::new("."),
            &MemCache::default(),
            None,
            &RunOptions::default(),
        )
        .await
        .err()
        .unwrap_or_else(|| panic!("{runner} must be refused"));
        assert!(err.is_config_error(), "{runner}: {err}");
        assert!(err.to_string().contains(path), "{runner}: {err}");
    }
}

/// A grader that passes everything and counts how often it was asked.
#[derive(Default)]
struct CountingGrader(AtomicU32);

#[async_trait]
impl AssertGrader for CountingGrader {
    async fn grade(
        &self,
        _assert: &crate::config::Assert,
        _output: &crate::types::Output,
        _ctx: &GradeCtx<'_>,
    ) -> Result<crate::cache::Graded, crate::errors::GraderError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(crate::cache::Graded::unpriced(
            crate::cache::GradedVerdict::Rubric {
                score: 1.0,
                pass: true,
                reasoning: "fine".into(),
            },
        ))
    }
}

/// Run a case whose deterministic assert fails, followed by a graded one,
/// under the given `runner:` block. Returns the case and the grader's calls.
async fn decided_case(runner_yaml: &str) -> (CaseResult, u32) {
    let suite = crate::load_str(&format!(
        r#"version: 1
providers:
  - id: p
    type: exec
    command: ["sh", "-c", "cat >/dev/null; printf '{{\"output\":\"hello\"}}'"]
tests:
  - id: t
    assert:
      - {{type: contains, value: MISSING}}
      - {{type: llm-rubric, value: good}}
runner: {runner_yaml}
"#
    ))
    .unwrap();
    let grader = CountingGrader::default();
    let mut result = run(
        &suite,
        Path::new("."),
        &MemCache::default(),
        Some(&grader),
        &RunOptions::default(),
    )
    .await
    .unwrap();
    (result.cases.remove(0), grader.0.load(Ordering::SeqCst))
}

/// The documented default: the deterministic fail decides the case, so the
/// judge is never paid and its assertion is reported `skipped`.
#[tokio::test]
async fn short_circuit_defaults_on() {
    let (case, graded) = decided_case("{}").await;
    assert_eq!(case.status, CaseStatus::Fail);
    assert_eq!(case.asserts[1].status, crate::result::AssertStatus::Skipped);
    assert_eq!(graded, 0);
}

/// `short_circuit: false` grades every assertion anyway — a full per-assertion
/// report — without changing the case's verdict.
#[tokio::test]
async fn short_circuit_false_grades_every_assertion() {
    let (case, graded) = decided_case("{short_circuit: false}").await;
    assert_eq!(graded, 1, "the judge must be asked");
    assert_eq!(case.asserts[1].status, crate::result::AssertStatus::Pass);
    assert_eq!(
        case.status,
        CaseStatus::Fail,
        "grading more never rescues a case the deterministic pass failed"
    );
}
