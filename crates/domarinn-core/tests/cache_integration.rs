//! End-to-end cache behavior: what gets reused, and what busts a reuse.
//!
//! Every ingredient of the provider cache key gets a test here — the provider
//! fingerprint (command and `cache_salt`), the rendered prompt, the vars, the
//! per-case `cache_salt`, and the repeat index — alongside the gates that decide
//! whether the cache is consulted at all (exec cacheability, cache modes,
//! latency asserts) and the separate grader-verdict cache.
//!
//! The property most of these defend is **locality**: changing one case must
//! bust that case and leave every other entry reusable. A suite-wide bust is
//! always correct but throws away work, which for an LLM-graded suite is paid
//! for in real money.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use async_trait::async_trait;
use domarinn_core::cache::{
    CacheBackend, CacheEntry, CacheError, CacheKey, CacheMode, CacheStats, PurgeFilter,
};
use domarinn_core::result::CaseStatus;
use domarinn_core::runner::{run, RunOptions};
use domarinn_core::{DefaultGrader, RunResult};
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// In-memory, first-write-wins cache mirroring the real backends' semantics.
#[derive(Default)]
struct MemCache {
    map: Mutex<HashMap<String, CacheEntry>>,
}

impl MemCache {
    fn entries(&self) -> usize {
        self.map.lock().unwrap().len()
    }
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
            entries: self.entries() as u64,
            ..Default::default()
        })
    }
    async fn purge(&self, _filter: &PurgeFilter) -> Result<u64, CacheError> {
        Ok(0)
    }
}

async fn run_suite(yaml: &str, opts: RunOptions, cache: &dyn CacheBackend) -> RunResult {
    let suite = domarinn_core::load_str(yaml).unwrap();
    run(&suite, Path::new("."), cache, None, &opts)
        .await
        .unwrap()
}

async fn run_default(yaml: &str, cache: &dyn CacheBackend) -> RunResult {
    run_suite(yaml, RunOptions::default(), cache).await
}

/// One test case in a generated suite.
struct Case<'a> {
    id: &'a str,
    var: &'a str,
    salt: Option<&'a str>,
}

impl<'a> Case<'a> {
    fn new(id: &'a str, var: &'a str) -> Self {
        Case {
            id,
            var,
            salt: None,
        }
    }
    fn salted(id: &'a str, var: &'a str, salt: &'a str) -> Self {
        Case {
            id,
            var,
            salt: Some(salt),
        }
    }
}

/// A suite of `cases` against one exec provider that prints `output`.
/// `provider_salt: None` leaves the provider uncacheable.
fn suite_with(provider_salt: Option<&str>, output: &str, cases: &[Case]) -> String {
    let salt_line = match provider_salt {
        Some(s) => format!("\n    cache_salt: \"{s}\""),
        None => String::new(),
    };
    let mut tests = String::new();
    for c in cases {
        tests.push_str(&format!(
            "  - id: {}\n    vars: {{x: \"{}\"}}\n",
            c.id, c.var
        ));
        if let Some(s) = c.salt {
            tests.push_str(&format!("    cache_salt: \"{s}\"\n"));
        }
    }
    format!(
        r#"
version: 1
project: test
suite: cache
providers:
  - id: p
    type: exec
    command: ["sh", "-c", "cat >/dev/null; printf '{{\"output\":\"{output}\"}}'"]{salt_line}
tests:
{tests}"#
    )
}

/// Two cases with distinct vars, neither salted — the ordinary baseline.
fn plain_two_case_suite() -> String {
    suite_with(
        Some("v1"),
        "out",
        &[Case::new("case-a", "a"), Case::new("case-b", "b")],
    )
}

// ── Reuse ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn unchanged_rerun_hits_every_case_and_writes_nothing_new() {
    let yaml = plain_two_case_suite();
    let cache = MemCache::default();

    let first = run_default(&yaml, &cache).await;
    assert_eq!(first.summary.cache_misses, 2);
    assert_eq!(first.summary.cache_hits, 0);
    assert_eq!(cache.entries(), 2);

    let second = run_default(&yaml, &cache).await;
    assert_eq!(second.summary.cache_hits, 2, "an unchanged rerun is free");
    assert_eq!(
        cache.entries(),
        2,
        "a fully-reused run must not write new entries"
    );
    assert!(second.cases.iter().all(|c| c.cached));
}

#[tokio::test]
async fn an_unrelated_env_change_does_not_bust() {
    // The request identity must exclude the process environment, or no two
    // machines would ever share a cache.
    let yaml = plain_two_case_suite();
    let cache = MemCache::default();
    run_default(&yaml, &cache).await;

    std::env::set_var("DOMARINN_CACHE_E2E_PROBE", "changed");
    let second = run_default(&yaml, &cache).await;
    std::env::remove_var("DOMARINN_CACHE_E2E_PROBE");

    assert_eq!(second.summary.cache_hits, 2);
    assert_eq!(cache.entries(), 2);
}

// ── Busting, one key ingredient at a time ────────────────────────────────────

#[tokio::test]
async fn changing_one_cases_var_busts_only_that_case() {
    let cache = MemCache::default();
    run_default(&plain_two_case_suite(), &cache).await;

    // case-a's var moves; case-b is untouched.
    let edited = suite_with(
        Some("v1"),
        "out",
        &[Case::new("case-a", "a-EDITED"), Case::new("case-b", "b")],
    );
    let second = run_default(&edited, &cache).await;

    assert_eq!(second.summary.cache_hits, 1, "case-b stays cached");
    assert_eq!(second.summary.cache_misses, 1, "only case-a re-pays");
    assert_eq!(cache.entries(), 3);
    let a = second
        .cases
        .iter()
        .find(|c| c.cell.test_id == "case-a")
        .unwrap();
    assert!(!a.cached);
}

#[tokio::test]
async fn per_case_salt_busts_only_that_case() {
    // The headline behavior: a per-prompt digest attached to one case must not
    // disturb any other case's entry.
    let before = suite_with(
        Some("v1"),
        "out",
        &[
            Case::salted("case-a", "a", "digest-1"),
            Case::salted("case-b", "b", "digest-1"),
        ],
    );
    let cache = MemCache::default();
    let first = run_default(&before, &cache).await;
    assert_eq!(first.summary.cache_misses, 2);
    assert_eq!(cache.entries(), 2);

    // Only case-a's prompt changed, so only its digest moves.
    let after = suite_with(
        Some("v1"),
        "out",
        &[
            Case::salted("case-a", "a", "digest-2"),
            Case::salted("case-b", "b", "digest-1"),
        ],
    );
    let second = run_default(&after, &cache).await;

    assert_eq!(second.summary.cache_hits, 1, "case-b must still be cached");
    assert_eq!(second.summary.cache_misses, 1, "only case-a re-pays");
    assert_eq!(cache.entries(), 3);
}

#[tokio::test]
async fn adding_a_salt_to_one_case_leaves_the_other_cached() {
    // Backward compatibility, end to end: introducing the feature for one case
    // must not invalidate entries written before any salt existed.
    let cache = MemCache::default();
    let first = run_default(&plain_two_case_suite(), &cache).await;
    assert_eq!(first.summary.cache_misses, 2);

    let salted = suite_with(
        Some("v1"),
        "out",
        &[
            Case::salted("case-a", "a", "newly-salted"),
            Case::new("case-b", "b"),
        ],
    );
    let second = run_default(&salted, &cache).await;

    assert_eq!(
        second.summary.cache_hits, 1,
        "the untouched, unsalted case must still hit"
    );
    assert_eq!(second.summary.cache_misses, 1);
}

#[tokio::test]
async fn identical_vars_collide_until_a_per_case_salt_separates_them() {
    // With no prompts block the key's `prompt` member is null, and `test.id`
    // never enters the key — so two cases with identical vars share one entry.
    // This is exactly the shape of an exec suite whose system under test
    // resolves its own prompt from the test id.
    let colliding = suite_with(
        Some("v1"),
        "out",
        &[Case::new("case-a", "same"), Case::new("case-b", "same")],
    );
    let cache = MemCache::default();
    let first = run_default(&colliding, &cache).await;
    assert_eq!(
        cache.entries(),
        1,
        "identical vars share a single entry without a salt"
    );
    assert_eq!(
        first.summary.cache_hits, 1,
        "the second case reuses the first"
    );

    let separated = suite_with(
        Some("v1"),
        "out",
        &[
            Case::salted("case-a", "same", "digest-a"),
            Case::salted("case-b", "same", "digest-b"),
        ],
    );
    let cache = MemCache::default();
    let second = run_default(&separated, &cache).await;
    assert_eq!(cache.entries(), 2, "distinct salts give distinct entries");
    assert_eq!(second.summary.cache_hits, 0);
}

#[tokio::test]
async fn changing_the_provider_salt_busts_every_case() {
    // The provider salt is the SUT version pin and lives in the shared
    // fingerprint, so bumping it is deliberately global.
    let cache = MemCache::default();
    run_default(&plain_two_case_suite(), &cache).await;

    let rebuilt = suite_with(
        Some("v2"),
        "out",
        &[Case::new("case-a", "a"), Case::new("case-b", "b")],
    );
    let second = run_default(&rebuilt, &cache).await;

    assert_eq!(
        second.summary.cache_hits, 0,
        "a rebuilt SUT re-pays in full"
    );
    assert_eq!(second.summary.cache_misses, 2);
    assert_eq!(cache.entries(), 4);
}

#[tokio::test]
async fn changing_the_provider_command_busts_every_case() {
    let cache = MemCache::default();
    run_default(&plain_two_case_suite(), &cache).await;

    // A different command is a different fingerprint, even at the same salt.
    let other = suite_with(
        Some("v1"),
        "different-output",
        &[Case::new("case-a", "a"), Case::new("case-b", "b")],
    );
    let second = run_default(&other, &cache).await;

    assert_eq!(second.summary.cache_hits, 0);
    assert_eq!(cache.entries(), 4);
}

#[tokio::test]
async fn changing_a_prompt_template_busts_only_cells_using_it() {
    // Two prompts crossed with one test give two cells; editing one template
    // must leave the other cell reusable.
    let suite_yaml = |a: &str, b: &str| {
        format!(
            r#"
version: 1
project: test
suite: cache
providers:
  - id: p
    type: exec
    command: ["sh", "-c", "cat >/dev/null; printf '{{\"output\":\"ok\"}}'"]
    cache_salt: "v1"
prompts:
  - id: pa
    template: "{a}"
  - id: pb
    template: "{b}"
tests:
  - id: t
    vars: {{x: "1"}}
"#
        )
    };

    let cache = MemCache::default();
    let first = run_default(&suite_yaml("alpha {{ x }}", "beta {{ x }}"), &cache).await;
    assert_eq!(first.cases.len(), 2);
    assert_eq!(cache.entries(), 2);

    let second = run_default(&suite_yaml("alpha EDITED {{ x }}", "beta {{ x }}"), &cache).await;
    assert_eq!(
        second.summary.cache_hits, 1,
        "the untouched prompt's cell stays cached"
    );
    assert_eq!(second.summary.cache_misses, 1);
    assert_eq!(cache.entries(), 3);
}

#[tokio::test]
async fn repeat_trials_get_independent_entries() {
    // Each repeat index is its own key, so N=2 cannot serve trial 1 from
    // trial 0 — otherwise sampling variance would collapse to one sample.
    let yaml = suite_with(Some("v1"), "out", &[Case::new("case-a", "a")]);
    let cache = MemCache::default();
    let opts = RunOptions {
        repeat: 2,
        ..Default::default()
    };

    let first = run_suite(&yaml, opts.clone(), &cache).await;
    assert_eq!(first.cases.len(), 2);
    assert_eq!(first.summary.cache_hits, 0);
    assert_eq!(cache.entries(), 2, "one entry per repeat index");

    let second = run_suite(&yaml, opts, &cache).await;
    assert_eq!(second.summary.cache_hits, 2);
}

// ── Gates: when the cache is not consulted at all ────────────────────────────

#[tokio::test]
async fn exec_without_a_provider_salt_is_never_cached() {
    // An exec fingerprint is just its command, which does not change when the
    // program behind it is rebuilt — so without a salt it must not be cached.
    let yaml = suite_with(None, "out", &[Case::new("case-a", "a")]);
    let cache = MemCache::default();

    run_default(&yaml, &cache).await;
    let second = run_default(&yaml, &cache).await;

    assert_eq!(second.summary.cache_hits, 0);
    assert_eq!(cache.entries(), 0, "nothing may be written");
}

#[tokio::test]
async fn a_per_case_salt_alone_does_not_enable_caching() {
    // The two salts answer different questions. A case salt says "this content
    // is unchanged"; only the provider salt says "this build is unchanged".
    let yaml = suite_with(None, "out", &[Case::salted("case-a", "a", "digest-1")]);
    let cache = MemCache::default();

    run_default(&yaml, &cache).await;
    let second = run_default(&yaml, &cache).await;

    assert_eq!(second.summary.cache_hits, 0);
    assert_eq!(cache.entries(), 0);
}

#[tokio::test]
async fn disabled_mode_neither_reads_nor_writes() {
    let yaml = plain_two_case_suite();
    let cache = MemCache::default();
    let opts = RunOptions {
        cache_mode: CacheMode::Disabled,
        ..Default::default()
    };

    run_suite(&yaml, opts.clone(), &cache).await;
    let second = run_suite(&yaml, opts, &cache).await;

    assert_eq!(second.summary.cache_hits, 0);
    assert_eq!(cache.entries(), 0);
}

#[tokio::test]
async fn cache_only_mode_errors_on_a_miss() {
    // Fully offline CI: a miss is an infrastructure failure, never a silent
    // live call.
    let yaml = plain_two_case_suite();
    let cache = MemCache::default();
    let result = run_suite(
        &yaml,
        RunOptions {
            cache_mode: CacheMode::ReadOnlyStrict,
            ..Default::default()
        },
        &cache,
    )
    .await;

    assert!(result.cases.iter().all(|c| c.status == CaseStatus::Error));
    assert!(result.cases[0]
        .error
        .as_deref()
        .unwrap_or_default()
        .contains("cache-only"));
}

#[tokio::test]
async fn cache_only_mode_succeeds_once_the_entry_is_warm() {
    let yaml = plain_two_case_suite();
    let cache = MemCache::default();
    run_default(&yaml, &cache).await;

    let strict = run_suite(
        &yaml,
        RunOptions {
            cache_mode: CacheMode::ReadOnlyStrict,
            ..Default::default()
        },
        &cache,
    )
    .await;
    assert_eq!(strict.summary.cache_hits, 2);
    assert!(strict.cases.iter().all(|c| c.status != CaseStatus::Error));
}

#[tokio::test]
async fn a_latency_assert_bypasses_the_cache() {
    // A cached latency is meaningless, so these cases must always call live.
    let yaml = r#"
version: 1
project: test
suite: cache
providers:
  - id: p
    type: exec
    command: ["sh", "-c", "cat >/dev/null; printf '{\"output\":\"ok\"}'"]
    cache_salt: "v1"
tests:
  - id: slow
    vars: {x: "1"}
    assert:
      - {type: latency, max: 60000}
"#;
    let cache = MemCache::default();

    run_default(yaml, &cache).await;
    let second = run_default(yaml, &cache).await;

    assert_eq!(second.summary.cache_hits, 0);
    assert_eq!(
        cache.entries(),
        0,
        "a latency-asserted case must not be stored"
    );
}

// ── The grader-verdict cache ─────────────────────────────────────────────────

/// Grader verdicts are **not** cached today. [`domarinn_core::runner::AssertGrader`]'s
/// `grade` takes no cache backend, and `DefaultGrader` calls its endpoint through
/// a bare `reqwest::Client`, so there is no path by which a verdict could be
/// reused. An LLM-graded suite therefore re-pays for every verdict on every run,
/// even when the provider response itself was a cache hit.
///
/// This pins the real behavior so the gap is visible rather than assumed away.
/// When verdict caching lands, the final assertion becomes `after_first` and this
/// test should be renamed.
#[tokio::test]
async fn grader_verdicts_are_not_cached_today() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "stop_reason": "tool_use",
            "content": [{
                "type": "tool_use", "name": "submit_verdict",
                "input": {"reasoning": "clear refusal", "pass": true, "score": 1.0}
            }]
        })))
        .mount(&server)
        .await;
    std::env::set_var("DOMARINN_CACHE_E2E_GRADER_KEY", "sk-test");

    let yaml = format!(
        r#"
version: 1
suite: graded
providers:
  - id: p
    type: exec
    command: ["sh", "-c", "cat >/dev/null; printf '{{\"output\":\"I cannot help\"}}'"]
    cache_salt: "v1"
grader:
  provider: {{type: anthropic, model: claude-x, base_url: "{uri}", api_key_env: DOMARINN_CACHE_E2E_GRADER_KEY}}
tests:
  - id: decline
    vars: {{}}
    assert:
      - {{type: llm-rubric, value: "declines the task"}}
"#,
        uri = server.uri()
    );
    let suite = domarinn_core::load_str(&yaml).unwrap();
    let grader = DefaultGrader::new(suite.grader.clone());
    let cache = MemCache::default();

    let first = run(
        &suite,
        Path::new("."),
        &cache,
        Some(&grader),
        &RunOptions::default(),
    )
    .await
    .unwrap();
    assert_eq!(first.cases[0].status, CaseStatus::Pass);
    let after_first = server.received_requests().await.unwrap().len();
    assert_eq!(after_first, 1, "the grader is called once on a cold run");

    let second = run(
        &suite,
        Path::new("."),
        &cache,
        Some(&grader),
        &RunOptions::default(),
    )
    .await
    .unwrap();
    assert_eq!(second.cases[0].status, CaseStatus::Pass);

    // The provider response *is* reused — the gap is specific to grading.
    assert_eq!(
        second.summary.cache_hits, 1,
        "the provider response should still come from cache"
    );
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        2,
        "known gap: the verdict is re-graded even though the response was cached"
    );
}
