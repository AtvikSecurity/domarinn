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
use std::path::{Path, PathBuf};
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
///
/// `provider_salt: None` is just an unsalted provider — it used to mean
/// "uncacheable", which is the rule these tests were originally written around.
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
async fn exec_is_cached_without_a_provider_salt() {
    // This was `exec_without_a_provider_salt_is_never_cached`, and asserted the
    // opposite. An exec fingerprint used to be argv alone, which does not move
    // when the program behind it is rebuilt, so declining to cache was the only
    // safe answer. Then the fingerprint carried the program's own identity, and
    // caching by default became safe — at the cost of making every exec key a
    // property of one machine's filesystem. Both are gone now: `command` names
    // what will answer, `cache_salt` says when that answer is stale, and a
    // rebuild is reported rather than pre-emptively charged for. See
    // `tests/cache_portability.rs` for the property that bought.
    let yaml = suite_with(None, "out", &[Case::new("case-a", "a")]);
    let cache = MemCache::default();

    run_default(&yaml, &cache).await;
    let second = run_default(&yaml, &cache).await;

    assert_eq!(second.summary.cache_hits, 1);
    assert_eq!(cache.entries(), 1);
}

#[tokio::test]
async fn a_per_case_salt_separates_entries_without_gating_them() {
    // The two salts answer different questions — a case salt says "this content
    // is unchanged", a provider salt says "this build is unchanged" — and
    // neither is what *enables* caching. A case salt chooses the key of an entry
    // that would have been written regardless.
    let yaml = suite_with(None, "out", &[Case::salted("case-a", "a", "digest-1")]);
    let cache = MemCache::default();

    run_default(&yaml, &cache).await;
    let second = run_default(&yaml, &cache).await;

    assert_eq!(second.summary.cache_hits, 1);
    assert_eq!(cache.entries(), 1);
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

/// A child that records one byte per call beside itself (`$0`), so a test can
/// count the live calls a run actually made. Same `$0`-sidecar trick as
/// [`write_flaky_child`], which needs no plumbing through the suite.
fn write_counting_child(dir: &Path) -> (String, PathBuf) {
    let path = dir.join("counting.sh");
    std::fs::write(
        &path,
        r#"cat >/dev/null
printf 'x' >> "$0.calls"
printf '{"output":"ok"}'
"#,
    )
    .unwrap();
    let calls = dir.join("counting.sh.calls");
    (path.to_string_lossy().into_owned(), calls)
}

/// How many times [`write_counting_child`] has been invoked. Absent file = zero,
/// so this reads correctly before the first call as well as after a reset.
fn live_calls(calls: &Path) -> usize {
    std::fs::read_to_string(calls).map(|s| s.len()).unwrap_or(0)
}

#[tokio::test]
async fn cache_only_errors_a_latency_case_instead_of_calling_live() {
    // `--cache-only` promises offline replay, and the credential preflight is
    // skipped on the strength of that promise. A latency assert forces a live
    // call — so under strict mode the honest answer is a per-case infra error,
    // not a silent trip to the provider. Per case, not per run: the refusal
    // must not take the rest of the suite with it.
    let dir = tempfile::tempdir().unwrap();
    let (script, calls) = write_counting_child(dir.path());
    let yaml = format!(
        r#"
version: 1
project: test
suite: cache
providers:
  - id: p
    type: exec
    command: ["sh", "{script}"]
    cache_salt: "v1"
tests:
  - id: replayable
    vars: {{x: "a"}}
  - id: timed
    vars: {{x: "b"}}
    assert:
      - {{type: latency, max: 60000}}
"#
    );
    let cache = MemCache::default();

    // Warm what can be warmed: a latency case bypasses the cache by design, so
    // only its sibling ever has an entry to replay.
    run_default(&yaml, &cache).await;
    assert_eq!(cache.entries(), 1, "a latency-asserted case is not stored");
    // Reset, so the count below is this run's live calls and no earlier one's.
    std::fs::write(&calls, "").unwrap();

    let strict = run_suite(
        &yaml,
        RunOptions {
            cache_mode: CacheMode::ReadOnlyStrict,
            ..Default::default()
        },
        &cache,
    )
    .await;

    let timed = strict
        .cases
        .iter()
        .find(|c| c.cell.test_id == "timed")
        .unwrap();
    assert_eq!(timed.status, CaseStatus::Error);
    assert_eq!(
        timed.error_class.as_ref().map(|c| c.as_str()),
        Some("cache_miss"),
        "reuses the cache-only class so CI needs no new vocabulary"
    );
    // Pinned whole: the class is shared with an ordinary strict-mode miss, so
    // the message is the only thing telling the two apart.
    assert_eq!(
        timed.error.as_deref(),
        Some(
            "cache-only: test 'timed' has a latency assert, which always \
             measures a live call; there is nothing honest to replay"
        )
    );

    let replayable = strict
        .cases
        .iter()
        .find(|c| c.cell.test_id == "replayable")
        .unwrap();
    assert_ne!(
        replayable.status,
        CaseStatus::Error,
        "one refused case must not fail its siblings"
    );
    assert_eq!(strict.summary.cache_hits, 1, "the sibling still replays");

    assert_eq!(
        live_calls(&calls),
        0,
        "--cache-only must reach the provider zero times"
    );
}

// ── Grading, through the cache ───────────────────────────────────────────────

/// Grader verdicts are reused across runs.
///
/// This test was `grader_verdicts_are_not_cached_today`, and pinned the
/// opposite: an LLM-graded suite re-paid its judge on every run even when every
/// provider response was a cache hit, which is the dominant recurring cost of
/// running one. Renamed rather than replaced so `git log -S` and `git blame`
/// point at the commit that closed the gap.
///
/// The mechanism underneath changed again in 0.5.0 — what is stored is the
/// judge's *exchange* rather than a bespoke verdict entry, keyed by the same
/// rule as every other cached call — and the invariant is deliberately written
/// so it did not have to: exactly one judge call across two runs, however that
/// is achieved. `grader_request_cache.rs` covers the how.
#[tokio::test]
async fn grader_verdicts_are_reused_across_runs() {
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

    assert_eq!(
        second.summary.cache_hits, 1,
        "the provider response should still come from cache"
    );
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        1,
        "the second run must reuse the verdict rather than re-pay the judge"
    );
    assert!(
        second.cases[0].asserts[0].cached,
        "…and must report that it did"
    );
}

// ── Retry × cache ────────────────────────────────────────────────────────────

/// A child that reports a **retriable** protocol error on its first call and
/// succeeds on every later one. The marker lives beside the script (`$0`), so
/// statefulness needs no extra plumbing through the suite.
fn write_flaky_child(dir: &Path) -> String {
    let path = dir.join("flaky.sh");
    std::fs::write(
        &path,
        r#"cat >/dev/null
if [ -f "$0.marker" ]; then
  printf '{"output":"ok"}'
else
  : > "$0.marker"
  printf '{"output":null,"error":{"message":"429 from upstream","retriable":true}}'
fi
"#,
    )
    .unwrap();
    path.to_string_lossy().into_owned()
}

/// A child that is retriable forever — used to prove an exhausted retry writes
/// nothing.
fn write_always_retriable_child(dir: &Path) -> String {
    let path = dir.join("always-429.sh");
    std::fs::write(
        &path,
        r#"cat >/dev/null
printf '{"output":null,"error":{"message":"429 from upstream","retriable":true}}'
"#,
    )
    .unwrap();
    path.to_string_lossy().into_owned()
}

/// One cacheable exec case backed by `script`, optionally with retries enabled.
/// Backoff is deliberately tiny so the test does not sleep for real.
fn script_suite(script: &str, retries: Option<u32>) -> String {
    script_suite_with_backoff(script, retries, 1)
}

/// As [`script_suite`], with an explicit backoff so a test can make the sleep
/// large enough to observe.
fn script_suite_with_backoff(script: &str, retries: Option<u32>, initial_ms: u64) -> String {
    let runner = match retries {
        Some(max) => format!(
            "\nrunner: {{retries: {{max: {max}, initial_ms: {initial_ms}, max_ms: {initial_ms}}}}}"
        ),
        None => String::new(),
    };
    format!(
        r#"
version: 1
project: test
suite: cache
providers:
  - id: p
    type: exec
    command: ["sh", "{script}"]
    cache_salt: "v1"
tests:
  - id: case-a
    vars: {{x: "a"}}{runner}
"#
    )
}

#[tokio::test]
async fn a_retried_call_caches_the_successful_attempt_exactly_once() {
    // The failed attempt must leave no trace: one entry, holding the response
    // that actually succeeded.
    let dir = tempfile::tempdir().unwrap();
    let yaml = script_suite(&write_flaky_child(dir.path()), Some(2));
    let cache = MemCache::default();

    let first = run_default(&yaml, &cache).await;
    assert_eq!(
        first.cases[0].attempts, 2,
        "the first attempt failed retriably, the second succeeded"
    );
    assert_eq!(first.cases[0].output.as_ref().unwrap().as_text(), "ok");
    assert_eq!(
        cache.entries(),
        1,
        "exactly one entry, from the good attempt"
    );

    let second = run_default(&yaml, &cache).await;
    assert_eq!(second.summary.cache_hits, 1);
    assert!(second.cases[0].cached);
    assert_eq!(second.cases[0].output.as_ref().unwrap().as_text(), "ok");
    assert_eq!(
        second.cases[0].attempts, 2,
        "a hit replays what the original call actually cost, not a 0 sentinel"
    );
    assert_eq!(cache.entries(), 1);
}

#[tokio::test]
async fn an_exhausted_retry_caches_nothing() {
    // A poisoned entry would be worse than the failure itself: every later run
    // would fail *from cache*, with no recovery but --no-cache.
    let dir = tempfile::tempdir().unwrap();
    let yaml = script_suite(&write_always_retriable_child(dir.path()), Some(1));
    let cache = MemCache::default();

    let result = run_default(&yaml, &cache).await;
    assert_eq!(result.cases[0].status, CaseStatus::Error);
    assert_eq!(cache.entries(), 0, "a failed call must never be cached");
}

#[tokio::test]
async fn an_empty_output_is_cached_and_replayed_like_any_other() {
    // Deliberately documents today's behavior: an empty response is a
    // *successful* response, so it is cached and replayed forever. That is what
    // makes a transient empty sticky, and it is the constraint `empty_output_mode`
    // has to reckon with — a mode of `error` must refuse to cache, not cache an
    // error. When that lands, this test should change alongside it.
    let yaml = suite_with(Some("v1"), "", &[Case::new("case-a", "a")]);
    let cache = MemCache::default();

    let first = run_default(&yaml, &cache).await;
    assert_eq!(first.cases[0].output.as_ref().unwrap().as_text(), "");
    assert_eq!(cache.entries(), 1);

    let second = run_default(&yaml, &cache).await;
    assert_eq!(second.summary.cache_hits, 1);
    assert_eq!(second.cases[0].output.as_ref().unwrap().as_text(), "");
}

#[test]
fn an_entry_written_by_a_newer_domarinn_still_deserializes() {
    // A shared S3/server cache is read by whatever version each teammate or CI
    // job happens to be running (docs/ci.md). An entry written by a newer
    // binary carries fields this one has never heard of; skipping them must be
    // a no-op, not a hard error that fails the whole run.
    let from_the_future = json!({
        "created_at": "2026-01-01T00:00:00Z",
        "provider_fingerprint": {"type": "exec"},
        "request": {"transport": "exec", "command": "./sut", "args": []},
        "output": "hi",
        "domarinn_version": "99.0.0",
        "empty_reason": "thinking_only",
        "reasoning": "let me work through this",
        "a_field_invented_after_this_binary_shipped": {"nested": true}
    });

    let entry: CacheEntry = serde_json::from_value(from_the_future).unwrap();
    assert_eq!(entry.output.as_text(), "hi");
    assert_eq!(entry.domarinn_version, "99.0.0");
    assert_eq!(entry.request.unwrap()["command"], json!("./sut"));
}

#[tokio::test]
async fn latency_excludes_retry_backoff_but_wall_time_includes_it() {
    // `AssertKind::Latency` reads `latency_ms` directly, so charging retry
    // backoff to it fails `{type: latency, max: N}` on a model that answered
    // fast — the moment one 429 fires. With retries on by default that is not a
    // corner case, it is every rate-limited suite.
    let dir = tempfile::tempdir().unwrap();
    let script = write_flaky_child(dir.path());
    // 300 ms is comfortably larger than a subprocess spawn, so the two numbers
    // are unambiguously distinguishable.
    let yaml = script_suite_with_backoff(&script, Some(2), 300);
    let cache = MemCache::default();

    let result = run_default(&yaml, &cache).await;
    let case = &result.cases[0];

    assert_eq!(case.attempts, 2, "one retriable failure, then success");
    let wall = case.wall_ms.expect("wall time is recorded");
    assert!(
        wall >= 300,
        "wall time must include the backoff sleep, was {wall}ms"
    );
    assert!(
        case.latency_ms < 250,
        "latency_ms must exclude the backoff, was {}ms",
        case.latency_ms
    );
}

#[tokio::test]
async fn a_run_option_overrides_the_suites_retry_budget() {
    // `--no-retries` has to win over a suite that asks for five, and it must do
    // so as a run option rather than by editing the suite: `config_digest` is
    // derived from the serialized suite, so mutating it would show a spurious
    // config drift in every `--against` comparison.
    let dir = tempfile::tempdir().unwrap();
    let yaml = script_suite(&write_always_retriable_child(dir.path()), Some(5));
    let cache = MemCache::default();

    let with_override = run_suite(
        &yaml,
        RunOptions {
            retries: Some(0),
            ..Default::default()
        },
        &cache,
    )
    .await;
    assert_eq!(
        with_override.cases[0].attempts, 1,
        "--no-retries wins over the suite's runner.retries.max"
    );

    let from_suite = run_default(&yaml, &cache).await;
    assert_eq!(
        from_suite.cases[0].attempts, 6,
        "5 retries plus the first try"
    );

    assert_eq!(
        with_override.config_digest, from_suite.config_digest,
        "a retry override must not perturb the config digest"
    );
}

#[tokio::test]
async fn retries_are_on_by_default() {
    // No `runner:` block at all. Before this default existed a single transient
    // failure scored the case 0.
    let dir = tempfile::tempdir().unwrap();
    let yaml = script_suite(&write_flaky_child(dir.path()), None);
    let cache = MemCache::default();

    let result = run_default(&yaml, &cache).await;
    assert_eq!(
        result.cases[0].attempts, 2,
        "the transient failure is retried without any configuration"
    );
    assert_eq!(result.cases[0].output.as_ref().unwrap().as_text(), "ok");
    assert_eq!(result.summary.retried_cases, 1, "and the run says so");
}
